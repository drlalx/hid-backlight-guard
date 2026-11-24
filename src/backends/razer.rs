//! Razer USB HID backlight backend ported from OpenRGB's `RazerController`.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use hidapi::{DeviceInfo as HidDeviceInfo, HidApi, HidDevice};

use crate::backends::razer_devices::{RAZER_DEVICES, RAZER_VID, RazerDeviceSpec, RazerMatrixType};
use crate::device::{
    BacklightBackend, BacklightDevice, DeviceInfo, DeviceKind, GuardError, RestoreMode, Vendor,
    write_u8,
};

const RAZER_REPORT_LEN: usize = 91;
const RAZER_STORAGE_NO_SAVE: u8 = 0x00;

const RAZER_LEVIATHAN_V2_PID: u16 = 0x0532;
const RAZER_LEVIATHAN_V2X_PID: u16 = 0x054A;

const RAZER_ORBWEAVER_CHROMA_PID: u16 = 0x0207;
const RAZER_TARTARUS_CHROMA_PID: u16 = 0x0208;
const RAZER_DEATHADDER_CHROMA_PID: u16 = 0x0043;
const RAZER_NAGA_EPIC_CHROMA_PID: u16 = 0x003E;
const RAZER_MAMBA_2012_WIRED_PID: u16 = 0x0024;
const RAZER_MAMBA_2012_WIRELESS_PID: u16 = 0x0025;

const RAZER_LED_SCROLL_WHEEL: u8 = 0x01;
const RAZER_LED_LOGO: u8 = 0x04;
const RAZER_LED_BACKLIGHT: u8 = 0x05;
const RAZER_ARGB_CHANNELS: [u8; 6] = [0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F];

type EffectCacheMap = HashMap<(String, u8), Vec<u8>>;

static SAVED_EXTENDED_EFFECTS: OnceLock<Mutex<EffectCacheMap>> = OnceLock::new();

fn saved_effects_map() -> &'static Mutex<EffectCacheMap> {
    SAVED_EXTENDED_EFFECTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Discovers and controls Razer keyboards, mice, keypads, headsets, mousemats, and accessories.
pub struct RazerBackend;

impl BacklightBackend for RazerBackend {
    fn name(&self) -> &'static str {
        "razer"
    }

    fn discover(&self, api: &HidApi) -> Result<Vec<Box<dyn BacklightDevice>>, GuardError> {
        let mut devices: Vec<Box<dyn BacklightDevice>> = Vec::new();
        let mut seen_paths: HashSet<String> = HashSet::new();
        let mut seen_pids: HashSet<(u16, Option<String>)> = HashSet::new();

        for dev_info in api.device_list() {
            if dev_info.vendor_id() != RAZER_VID {
                continue;
            }

            let Some(spec) = find_matching_spec(dev_info) else {
                continue;
            };

            let path = dev_info.path().to_string_lossy().into_owned();
            if !seen_paths.insert(path.clone()) {
                continue;
            }

            let serial_key = (
                spec.pid,
                dev_info
                    .serial_number()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
            );
            if !seen_pids.insert(serial_key) {
                continue;
            }

            let Ok(handle) = api.open_path(dev_info.path()) else {
                continue;
            };

            let report_id = report_index_for_pid(spec.pid);
            let led_ids = resolve_led_ids(&handle, report_id, spec);
            let led_summary: Vec<String> = led_ids.iter().map(|id| format!("0x{id:02X}")).collect();
            let info = DeviceInfo {
                vendor: Vendor::Razer,
                name: spec.name.to_owned(),
                kind: spec.kind,
                vid: RAZER_VID,
                pid: spec.pid,
                path,
                protocol: format!(
                    "Razer {:?} (tx=0x{:02X}, leds=[{}])",
                    spec.matrix,
                    spec.tx_id,
                    led_summary.join(",")
                ),
            };

            devices.push(Box::new(RazerBacklightDevice {
                handle,
                spec,
                report_id,
                led_ids,
                info,
            }));
        }

        Ok(devices)
    }
}

struct RazerBacklightDevice {
    handle: HidDevice,
    spec: RazerDeviceSpec,
    report_id: u8,
    led_ids: Vec<u8>,
    info: DeviceInfo,
}

impl BacklightDevice for RazerBacklightDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn set_backlight_off(&mut self) -> Result<(), GuardError> {
        match self.spec.matrix {
            RazerMatrixType::Standard | RazerMatrixType::Linear => {
                for &led_id in &self.led_ids {
                    self.send_report(&build_brightness_standard_report(
                        self.report_id,
                        self.spec.tx_id,
                        RAZER_STORAGE_NO_SAVE,
                        led_id,
                        0,
                    )?)?;
                }
            }
            RazerMatrixType::Extended => {
                for &led_id in &self.led_ids {
                    self.cache_current_extended_effect(led_id);
                    self.send_report(&build_brightness_extended_report(
                        self.report_id,
                        self.spec.tx_id,
                        RAZER_STORAGE_NO_SAVE,
                        led_id,
                        0,
                    )?)?;
                }
            }
            RazerMatrixType::ExtendedArgb => {
                for ch in RAZER_ARGB_CHANNELS {
                    self.cache_current_extended_effect(ch);
                    self.send_report(&build_brightness_extended_report(
                        self.report_id,
                        self.spec.tx_id,
                        RAZER_STORAGE_NO_SAVE,
                        ch,
                        0,
                    )?)?;
                }
            }
            RazerMatrixType::Custom => {
                self.set_custom_off()?;
            }
            RazerMatrixType::None => {
                for &led_id in &self.led_ids {
                    self.send_report(&build_brightness_standard_report(
                        self.report_id,
                        self.spec.tx_id,
                        RAZER_STORAGE_NO_SAVE,
                        led_id,
                        0,
                    )?)?;
                }
            }
        }
        Ok(())
    }

    fn set_backlight_on(&mut self, restore: RestoreMode) -> Result<(), GuardError> {
        match restore {
            RestoreMode::Native => {
                self.set_brightness_full()?;
                self.restore_extended_effect_if_off()?;
                Ok(())
            }
            RestoreMode::Static { r, g, b } => {
                self.set_static_color(r, g, b)?;
                self.set_brightness_full()
            }
        }
    }

    fn probe_details(&mut self) -> Result<Vec<String>, GuardError> {
        let mut lines = Vec::new();
        let regions = query_lighting_regions(&self.handle, self.report_id, self.spec.tx_id);
        if regions.is_empty() {
            lines.push("Regions (0x0F:0x80): <none / unsupported>".to_owned());
        } else {
            let hex: Vec<String> = regions.iter().map(|r| format!("0x{r:02X}")).collect();
            lines.push(format!("Regions (0x0F:0x80): [{}]", hex.join(", ")));
        }
        let mut probed = self.led_ids.clone();
        for fallback in [0x00u8, 0x01, 0x04, 0x05, 0x11] {
            if !probed.contains(&fallback) {
                probed.push(fallback);
            }
        }
        if let Some(dm) = self.query_device_mode() {
            lines.push(format!("Device Mode (0x00:0x84): 0x{dm:02X}"));
        }
        for led in probed {
            if let Some(br) = self.query_brightness_extended(led) {
                lines.push(format!("Brightness Extended (led=0x{led:02X}): {br}"));
            }
            if let Some(eff) = self.query_effect_extended(led) {
                let hex: Vec<String> = eff.iter().map(|b| format!("{b:02X}")).collect();
                lines.push(format!(
                    "Effect Extended (led=0x{led:02X}): [{}]",
                    hex.join(" ")
                ));
            }
        }
        if let Some(br_std) = self.query_brightness_standard(self.spec.led_id) {
            lines.push(format!(
                "Brightness Standard (led=0x{:02X}): {br_std}",
                self.spec.led_id
            ));
        }
        Ok(lines)
    }
}

impl RazerBacklightDevice {
    fn cache_current_extended_effect(&self, led_id: u8) {
        if let Some(effect) = self.query_effect_extended(led_id) {
            if effect.get(2).copied().unwrap_or(0) != 0x00 {
                if let Ok(mut guard) = saved_effects_map().lock() {
                    guard.insert((self.info.path.clone(), led_id), effect);
                }
            }
        }
    }

    fn restore_extended_effect_if_off(&self) -> Result<(), GuardError> {
        let leds: Vec<u8> = match self.spec.matrix {
            RazerMatrixType::Extended => self.led_ids.clone(),
            RazerMatrixType::ExtendedArgb => RAZER_ARGB_CHANNELS.to_vec(),
            _ => return Ok(()),
        };

        for led_id in leds {
            let current_effect = self
                .query_effect_extended(led_id)
                .and_then(|eff| eff.get(2).copied());

            if current_effect == Some(0x00) {
                let cached = saved_effects_map()
                    .lock()
                    .ok()
                    .and_then(|guard| guard.get(&(self.info.path.clone(), led_id)).cloned());

                if let Some(mut args) = cached {
                    write_u8(&mut args, 0, RAZER_STORAGE_NO_SAVE)?;
                    write_u8(&mut args, 1, led_id)?;
                    let data_size = u8::try_from(args.len().max(6)).unwrap_or(6);
                    let rep = build_report(
                        self.report_id,
                        self.spec.tx_id,
                        0x0F,
                        0x02,
                        data_size,
                        &args,
                    )?;
                    self.send_report(&rep)?;
                } else {
                    let rep = build_mode_spectrum_cycle_extended_report(
                        self.report_id,
                        self.spec.tx_id,
                        RAZER_STORAGE_NO_SAVE,
                        led_id,
                    )?;
                    self.send_report(&rep)?;
                }
            }
        }
        Ok(())
    }

    fn query_report(&self, report: &[u8; RAZER_REPORT_LEN]) -> Option<[u8; RAZER_REPORT_LEN]> {
        query_razer_report(&self.handle, self.report_id, report)
    }

    fn query_device_mode(&self) -> Option<u8> {
        let req = build_report(self.report_id, self.spec.tx_id, 0x00, 0x84, 0x02, &[]).ok()?;
        let resp = self.query_report(&req)?;
        if resp.get(1).copied() == Some(0x02) {
            resp.get(9).copied()
        } else {
            None
        }
    }

    fn query_effect_extended(&self, led_id: u8) -> Option<Vec<u8>> {
        let req = build_report(
            self.report_id,
            self.spec.tx_id,
            0x0F,
            0x82,
            0x06,
            &[RAZER_STORAGE_NO_SAVE, led_id],
        )
        .ok()?;
        let resp = self.query_report(&req)?;
        if resp.get(1).copied() == Some(0x02) {
            let size = usize::from(resp.get(6).copied().unwrap_or(6)).min(16);
            Some(resp.get(9..9 + size).unwrap_or(&[]).to_vec())
        } else {
            None
        }
    }

    fn query_brightness_extended(&self, led_id: u8) -> Option<u8> {
        let req = build_report(
            self.report_id,
            self.spec.tx_id,
            0x0F,
            0x84,
            0x03,
            &[RAZER_STORAGE_NO_SAVE, led_id, 0x00],
        )
        .ok()?;
        let resp = self.query_report(&req)?;
        if resp.get(1).copied() == Some(0x02) {
            resp.get(11).copied()
        } else {
            None
        }
    }

    fn query_brightness_standard(&self, led_id: u8) -> Option<u8> {
        let req = build_report(
            self.report_id,
            self.spec.tx_id,
            0x03,
            0x83,
            0x03,
            &[RAZER_STORAGE_NO_SAVE, led_id, 0x00],
        )
        .ok()?;
        let resp = self.query_report(&req)?;
        if resp.get(1).copied() == Some(0x02) {
            resp.get(11).copied()
        } else {
            None
        }
    }

    fn send_report(&self, report: &[u8; RAZER_REPORT_LEN]) -> Result<(), GuardError> {
        self.handle.send_feature_report(report)?;
        Ok(())
    }

    fn set_brightness_full(&self) -> Result<(), GuardError> {
        match self.spec.matrix {
            RazerMatrixType::Standard | RazerMatrixType::Linear => {
                for &led_id in &self.led_ids {
                    self.send_report(&build_brightness_standard_report(
                        self.report_id,
                        self.spec.tx_id,
                        RAZER_STORAGE_NO_SAVE,
                        led_id,
                        255,
                    )?)?;
                }
            }
            RazerMatrixType::Extended => {
                for &led_id in &self.led_ids {
                    self.send_report(&build_brightness_extended_report(
                        self.report_id,
                        self.spec.tx_id,
                        RAZER_STORAGE_NO_SAVE,
                        led_id,
                        255,
                    )?)?;
                }
            }
            RazerMatrixType::ExtendedArgb => {
                for ch in RAZER_ARGB_CHANNELS {
                    self.send_report(&build_brightness_extended_report(
                        self.report_id,
                        self.spec.tx_id,
                        RAZER_STORAGE_NO_SAVE,
                        ch,
                        255,
                    )?)?;
                }
            }
            RazerMatrixType::Custom => {
                self.set_custom_on()?;
            }
            RazerMatrixType::None => {
                for &led_id in &self.led_ids {
                    self.send_report(&build_brightness_standard_report(
                        self.report_id,
                        self.spec.tx_id,
                        RAZER_STORAGE_NO_SAVE,
                        led_id,
                        255,
                    )?)?;
                }
            }
        }
        Ok(())
    }

    fn set_static_color(&self, r: u8, g: u8, b: u8) -> Result<(), GuardError> {
        match self.spec.matrix {
            RazerMatrixType::Standard | RazerMatrixType::Linear => {
                self.send_report(&build_mode_static_standard_report(
                    self.report_id,
                    self.spec.tx_id,
                    r,
                    g,
                    b,
                )?)?;
            }
            RazerMatrixType::Extended | RazerMatrixType::ExtendedArgb => {
                for &led_id in &self.led_ids {
                    self.send_report(&build_mode_static_extended_report(
                        self.report_id,
                        self.spec.tx_id,
                        RAZER_STORAGE_NO_SAVE,
                        led_id,
                        r,
                        g,
                        b,
                    )?)?;
                }
            }
            RazerMatrixType::Custom => {
                self.set_custom_color(r, g, b)?;
            }
            RazerMatrixType::None => {}
        }
        Ok(())
    }

    fn set_custom_off(&self) -> Result<(), GuardError> {
        match self.spec.pid {
            RAZER_ORBWEAVER_CHROMA_PID => {
                self.send_report(&build_brightness_standard_report(
                    self.report_id,
                    self.spec.tx_id,
                    RAZER_STORAGE_NO_SAVE,
                    self.spec.led_id,
                    0,
                )?)?;
            }
            RAZER_TARTARUS_CHROMA_PID => {
                self.send_report(&build_set_led_effect_report(
                    self.report_id,
                    self.spec.tx_id,
                    RAZER_STORAGE_NO_SAVE,
                    RAZER_LED_BACKLIGHT,
                    0,
                )?)?;
                self.send_report(&build_set_led_rgb_report(
                    self.report_id,
                    self.spec.tx_id,
                    RAZER_STORAGE_NO_SAVE,
                    RAZER_LED_BACKLIGHT,
                    0,
                    0,
                    0,
                )?)?;
            }
            RAZER_DEATHADDER_CHROMA_PID | RAZER_NAGA_EPIC_CHROMA_PID => {
                let second_led = if self.spec.pid == RAZER_NAGA_EPIC_CHROMA_PID {
                    RAZER_LED_BACKLIGHT
                } else {
                    RAZER_LED_LOGO
                };
                for led in [RAZER_LED_SCROLL_WHEEL, second_led] {
                    self.send_report(&build_set_led_effect_report(
                        self.report_id,
                        self.spec.tx_id,
                        RAZER_STORAGE_NO_SAVE,
                        led,
                        0,
                    )?)?;
                    self.send_report(&build_set_led_rgb_report(
                        self.report_id,
                        self.spec.tx_id,
                        RAZER_STORAGE_NO_SAVE,
                        led,
                        0,
                        0,
                        0,
                    )?)?;
                }
            }
            RAZER_MAMBA_2012_WIRED_PID | RAZER_MAMBA_2012_WIRELESS_PID => {
                self.send_report(&build_set_led_effect_report(
                    self.report_id,
                    self.spec.tx_id,
                    RAZER_STORAGE_NO_SAVE,
                    RAZER_LED_SCROLL_WHEEL,
                    0,
                )?)?;
                self.send_report(&build_set_led_rgb_report(
                    self.report_id,
                    self.spec.tx_id,
                    RAZER_STORAGE_NO_SAVE,
                    RAZER_LED_SCROLL_WHEEL,
                    0,
                    0,
                    0,
                )?)?;
            }
            _ => {
                self.send_report(&build_brightness_standard_report(
                    self.report_id,
                    self.spec.tx_id,
                    RAZER_STORAGE_NO_SAVE,
                    self.spec.led_id,
                    0,
                )?)?;
            }
        }
        Ok(())
    }

    fn set_custom_on(&self) -> Result<(), GuardError> {
        match self.spec.pid {
            RAZER_ORBWEAVER_CHROMA_PID => {
                self.send_report(&build_brightness_standard_report(
                    self.report_id,
                    self.spec.tx_id,
                    RAZER_STORAGE_NO_SAVE,
                    self.spec.led_id,
                    255,
                )?)?;
            }
            _ => self.set_custom_color(255, 255, 255)?,
        }
        Ok(())
    }

    fn set_custom_color(&self, r: u8, g: u8, b: u8) -> Result<(), GuardError> {
        match self.spec.pid {
            RAZER_ORBWEAVER_CHROMA_PID => {
                self.send_report(&build_mode_static_standard_report(
                    self.report_id,
                    self.spec.tx_id,
                    r,
                    g,
                    b,
                )?)?;
            }
            RAZER_TARTARUS_CHROMA_PID => {
                self.send_report(&build_set_led_rgb_report(
                    self.report_id,
                    self.spec.tx_id,
                    RAZER_STORAGE_NO_SAVE,
                    RAZER_LED_BACKLIGHT,
                    r,
                    g,
                    b,
                )?)?;
                self.send_report(&build_set_led_effect_report(
                    self.report_id,
                    self.spec.tx_id,
                    RAZER_STORAGE_NO_SAVE,
                    RAZER_LED_BACKLIGHT,
                    0,
                )?)?;
            }
            RAZER_DEATHADDER_CHROMA_PID | RAZER_NAGA_EPIC_CHROMA_PID => {
                let second_led = if self.spec.pid == RAZER_NAGA_EPIC_CHROMA_PID {
                    RAZER_LED_BACKLIGHT
                } else {
                    RAZER_LED_LOGO
                };
                for led in [RAZER_LED_SCROLL_WHEEL, second_led] {
                    self.send_report(&build_set_led_rgb_report(
                        self.report_id,
                        self.spec.tx_id,
                        RAZER_STORAGE_NO_SAVE,
                        led,
                        r,
                        g,
                        b,
                    )?)?;
                    self.send_report(&build_set_led_effect_report(
                        self.report_id,
                        self.spec.tx_id,
                        RAZER_STORAGE_NO_SAVE,
                        led,
                        0,
                    )?)?;
                }
            }
            RAZER_MAMBA_2012_WIRED_PID | RAZER_MAMBA_2012_WIRELESS_PID => {
                self.send_report(&build_set_led_rgb_report(
                    self.report_id,
                    self.spec.tx_id,
                    RAZER_STORAGE_NO_SAVE,
                    RAZER_LED_SCROLL_WHEEL,
                    r,
                    g,
                    b,
                )?)?;
                self.send_report(&build_set_led_effect_report(
                    self.report_id,
                    self.spec.tx_id,
                    RAZER_STORAGE_NO_SAVE,
                    RAZER_LED_SCROLL_WHEEL,
                    0,
                )?)?;
            }
            _ => {}
        }
        Ok(())
    }
}

fn resolve_led_ids(handle: &HidDevice, report_id: u8, spec: RazerDeviceSpec) -> Vec<u8> {
    if matches!(spec.matrix, RazerMatrixType::Extended) {
        let regions = query_lighting_regions(handle, report_id, spec.tx_id);
        if !regions.is_empty() {
            return regions;
        }
    }
    vec![spec.led_id]
}

fn query_razer_report(
    handle: &HidDevice,
    report_id: u8,
    report: &[u8; RAZER_REPORT_LEN],
) -> Option<[u8; RAZER_REPORT_LEN]> {
    for _ in 0..2u8 {
        std::thread::sleep(std::time::Duration::from_millis(2));
        if handle.send_feature_report(report).is_err() {
            continue;
        }
        for _ in 0..25u8 {
            std::thread::sleep(std::time::Duration::from_millis(2));
            let mut resp = [0u8; RAZER_REPORT_LEN];
            if let Some(slot) = resp.get_mut(0) {
                *slot = report_id;
            }
            if handle.get_feature_report(&mut resp).is_err() {
                break;
            }
            let status = resp.get(1).copied().unwrap_or(0);
            let tx = resp.get(2).copied().unwrap_or(0);
            let cls = resp.get(7).copied().unwrap_or(0);
            let cmd = resp.get(8).copied().unwrap_or(0);
            if tx == report.get(2).copied().unwrap_or(0)
                && cls == report.get(7).copied().unwrap_or(0)
                && cmd == report.get(8).copied().unwrap_or(0)
                && status != 0x00
                && status != 0x01
            {
                return Some(resp);
            }
        }
    }
    None
}

fn query_lighting_regions(handle: &HidDevice, report_id: u8, tx_id: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let Ok(req) = build_report(report_id, tx_id, 0x0F, 0x80, 0x50, &[]) else {
        return out;
    };
    let Some(resp) = query_razer_report(handle, report_id, &req) else {
        return out;
    };
    if resp.get(1).copied() != Some(0x02) {
        return out;
    }
    let mut offset = 9usize;
    while offset + 4 < 89 && out.len() < 16 {
        let led = resp.get(offset).copied().unwrap_or(0);
        if led == 0x00 {
            break;
        }
        out.push(led);
        offset += 5;
    }
    out
}

fn find_matching_spec(dev_info: &HidDeviceInfo) -> Option<RazerDeviceSpec> {
    let pid = dev_info.product_id();
    for spec in RAZER_DEVICES {
        if spec.pid != pid {
            continue;
        }
        if let Some(expected_iface) = spec.interface {
            if dev_info.interface_number() != expected_iface {
                continue;
            }
        }
        if let Some(expected_up) = spec.usage_page {
            if dev_info.usage_page() != expected_up {
                continue;
            }
        }
        if let Some(expected_u) = spec.usage {
            if dev_info.usage() != expected_u {
                continue;
            }
        }
        return Some(*spec);
    }

    // Fallback for unlisted Razer HID mouse/keyboard control interfaces on Windows.
    if dev_info.usage_page() == 0x0001 && dev_info.usage() == 0x0002 {
        return Some(RazerDeviceSpec {
            name: "Razer USB HID Device",
            pid,
            kind: DeviceKind::Unknown,
            matrix: RazerMatrixType::Extended,
            tx_id: 0x3F,
            led_id: RAZER_LED_BACKLIGHT,
            interface: Some(dev_info.interface_number()),
            usage_page: Some(0x0001),
            usage: Some(0x0002),
        });
    }

    None
}

#[inline]
fn report_index_for_pid(pid: u16) -> u8 {
    match pid {
        RAZER_LEVIATHAN_V2_PID | RAZER_LEVIATHAN_V2X_PID => 0x07,
        _ => 0x00,
    }
}

/// Computes OpenRGB's Razer XOR CRC over report bytes `3..89` (`status..arguments`).
pub fn calculate_crc(report: &[u8; RAZER_REPORT_LEN]) -> u8 {
    let mut crc = 0u8;
    if let Some(slice) = report.get(3..89) {
        for &byte in slice {
            crc ^= byte;
        }
    }
    crc
}

pub fn build_report(
    report_id: u8,
    tx_id: u8,
    command_class: u8,
    command_id: u8,
    data_size: u8,
    args: &[u8],
) -> Result<[u8; RAZER_REPORT_LEN], GuardError> {
    let mut buf = [0u8; RAZER_REPORT_LEN];
    write_u8(&mut buf, 0, report_id)?;
    write_u8(&mut buf, 1, 0x00)?; // status: NEW
    write_u8(&mut buf, 2, tx_id)?;
    write_u8(&mut buf, 6, data_size)?;
    write_u8(&mut buf, 7, command_class)?;
    write_u8(&mut buf, 8, command_id)?;

    for (offset, &val) in args.iter().take(80).enumerate() {
        write_u8(&mut buf, 9 + offset, val)?;
    }

    let crc = calculate_crc(&buf);
    write_u8(&mut buf, 89, crc)?;
    Ok(buf)
}

pub fn build_brightness_standard_report(
    report_id: u8,
    tx_id: u8,
    storage: u8,
    led_id: u8,
    brightness: u8,
) -> Result<[u8; RAZER_REPORT_LEN], GuardError> {
    build_report(
        report_id,
        tx_id,
        0x03,
        0x03,
        0x03,
        &[storage, led_id, brightness],
    )
}

pub fn build_brightness_extended_report(
    report_id: u8,
    tx_id: u8,
    storage: u8,
    led_id: u8,
    brightness: u8,
) -> Result<[u8; RAZER_REPORT_LEN], GuardError> {
    build_report(
        report_id,
        tx_id,
        0x0F,
        0x04,
        0x03,
        &[storage, led_id, brightness],
    )
}

pub fn build_mode_spectrum_cycle_extended_report(
    report_id: u8,
    tx_id: u8,
    storage: u8,
    led_id: u8,
) -> Result<[u8; RAZER_REPORT_LEN], GuardError> {
    build_report(report_id, tx_id, 0x0F, 0x02, 0x06, &[storage, led_id, 0x03])
}

pub fn build_mode_static_standard_report(
    report_id: u8,
    tx_id: u8,
    r: u8,
    g: u8,
    b: u8,
) -> Result<[u8; RAZER_REPORT_LEN], GuardError> {
    build_report(report_id, tx_id, 0x03, 0x0A, 0x04, &[0x06, r, g, b])
}

pub fn build_mode_static_extended_report(
    report_id: u8,
    tx_id: u8,
    storage: u8,
    led_id: u8,
    r: u8,
    g: u8,
    b: u8,
) -> Result<[u8; RAZER_REPORT_LEN], GuardError> {
    build_report(
        report_id,
        tx_id,
        0x0F,
        0x02,
        0x09,
        &[storage, led_id, 0x01, 0x00, 0x00, 0x01, r, g, b],
    )
}

pub fn build_set_led_rgb_report(
    report_id: u8,
    tx_id: u8,
    storage: u8,
    led_id: u8,
    r: u8,
    g: u8,
    b: u8,
) -> Result<[u8; RAZER_REPORT_LEN], GuardError> {
    build_report(
        report_id,
        tx_id,
        0x03,
        0x01,
        0x05,
        &[storage, led_id, r, g, b],
    )
}

pub fn build_set_led_effect_report(
    report_id: u8,
    tx_id: u8,
    storage: u8,
    led_id: u8,
    effect: u8,
) -> Result<[u8; RAZER_REPORT_LEN], GuardError> {
    build_report(
        report_id,
        tx_id,
        0x03,
        0x02,
        0x03,
        &[storage, led_id, effect],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_brightness_report_matches_openrgb_layout() {
        let report = build_brightness_extended_report(0x00, 0x1F, 0x00, 0x05, 0x00).unwrap();
        assert_eq!(report.len(), 91);
        assert_eq!(report[0], 0x00);
        assert_eq!(report[2], 0x1F);
        assert_eq!(report[6], 0x03);
        assert_eq!(report[7], 0x0F);
        assert_eq!(report[8], 0x04);
        assert_eq!(report[9], 0x00);
        assert_eq!(report[10], 0x05);
        assert_eq!(report[11], 0x00);
        // XOR of 0x03 ^ 0x0F ^ 0x04 ^ 0x05 == 0x0D
        assert_eq!(report[89], 0x03 ^ 0x0F ^ 0x04 ^ 0x05);
        assert_eq!(report[90], 0x00);
    }

    #[test]
    fn standard_brightness_report_matches_openrgb_layout() {
        let report = build_brightness_standard_report(0x00, 0x3F, 0x00, 0x05, 0xFF).unwrap();
        assert_eq!(report[2], 0x3F);
        assert_eq!(report[6], 0x03);
        assert_eq!(report[7], 0x03);
        assert_eq!(report[8], 0x03);
        assert_eq!(report[9], 0x00);
        assert_eq!(report[10], 0x05);
        assert_eq!(report[11], 0xFF);
        assert_eq!(report[89], 0x03 ^ 0x05 ^ 0xFF);
    }

    #[test]
    fn ornata_v2_and_naga_trinity_are_registered() {
        assert!(RAZER_DEVICES.iter().any(|d| d.pid == 0x025D));
        assert!(RAZER_DEVICES.iter().any(|d| d.pid == 0x0067));
    }

    #[test]
    fn extended_spectrum_cycle_report_matches_openrgb_layout() {
        let report = build_mode_spectrum_cycle_extended_report(0x00, 0x1F, 0x00, 0x05).unwrap();
        assert_eq!(report[2], 0x1F);
        assert_eq!(report[6], 0x06);
        assert_eq!(report[7], 0x0F);
        assert_eq!(report[8], 0x02);
        assert_eq!(report[9], 0x00);
        assert_eq!(report[10], 0x05);
        assert_eq!(report[11], 0x03);
    }
}
