//! Logitech HID++ 2.0 and legacy Lightsync/G-series backlight backend ported from OpenRGB.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use hidapi::{DeviceInfo as HidDeviceInfo, HidApi, HidDevice};

use crate::device::{
    BacklightBackend, BacklightDevice, DeviceInfo, DeviceKind, GuardError, RestoreMode, Vendor,
    copy_into_slice, write_u8,
};

/// Logitech USB Vendor ID (`0x046D`).
pub const LOGITECH_VID: u16 = 0x046D;

const HIDPP_SHORT_REPORT_ID: u8 = 0x10;
const HIDPP_LONG_REPORT_ID: u8 = 0x11;
const HIDPP_VERY_LONG_REPORT_ID: u8 = 0x12;
const HIDPP_LONG_LEN: usize = 20;
const HIDPP_MAX_READ_LEN: usize = 64;

const HIDPP20_DEFAULT_DEVICE_INDEX: u8 = 0xFF;
const HIDPP20_SW_ID: u8 = 0x07;
const HIDPP20_ERROR_FEAT: u8 = 0xFF;
const HIDPP20_ERR_BUSY: u8 = 0x08;

const FEAT_FEATURE_SET: u16 = 0x0001;
const FEAT_DEVICE_NAME: u16 = 0x0005;
const FEAT_HEADSET_RGB_HOSTMODE: u16 = 0x0620;
const FEAT_CENTURION_RGB: u16 = 0x0600;
const FEAT_COLOR_LED_EFFECTS: u16 = 0x8070;
const FEAT_RGB_EFFECTS: u16 = 0x8071;
const FEAT_PER_KEY_V1: u16 = 0x8080;
const FEAT_PER_KEY_V2: u16 = 0x8081;

// Receiver / dongle PIDs that forward HID++ 2.0 frames on slots 1..=6.
const RECEIVER_PIDS: &[u16] = &[
    0xC52B, // Unifying
    0xC532, // Unifying Nano
    0xC534, // Nano
    0xC539, // Lightspeed
    0xC53A, // Powerplay
    0xC53D, // Lightspeed (G Pro / G903 Hero)
    0xC53F, // Lightspeed (G915 / G903)
    0xC541, // Lightspeed (G915)
    0xC545, // Lightspeed (G733 / Superlight)
    0xC547, // Lightspeed (Pro X)
    0xC548, // Bolt
    0xC54D, // Lightspeed
];

/// Discovers Logitech RGB peripherals over Unified HID++ 2.0 and legacy Logitech HID protocols.
pub struct LogitechBackend;

impl BacklightBackend for LogitechBackend {
    fn name(&self) -> &'static str {
        "logitech"
    }

    fn discover(&self, api: &HidApi) -> Result<Vec<Box<dyn BacklightDevice>>, GuardError> {
        let mut devices: Vec<Box<dyn BacklightDevice>> = Vec::new();
        let mut seen_paths: HashSet<String> = HashSet::new();
        let mut seen_nodes: HashSet<(u16, u8, String)> = HashSet::new();

        for dev_info in api.device_list() {
            if dev_info.vendor_id() != LOGITECH_VID || !is_logitech_control_node(dev_info) {
                continue;
            }

            let path = dev_info.path().to_string_lossy().into_owned();
            if !seen_paths.insert(path.clone()) {
                continue;
            }

            let Ok(handle) = api.open_path(dev_info.path()) else {
                continue;
            };

            let pid = dev_info.product_id();
            let usage_page = dev_info.usage_page();
            let usage = dev_info.usage();

            // 1. Try direct HID++ 2.0 on device_index = 0xFF.
            if let Some(dev) =
                probe_hidpp20_slot(dev_info, &path, HIDPP20_DEFAULT_DEVICE_INDEX, handle)
            {
                let serial = dev_info.serial_number().unwrap_or_default().to_owned();
                if seen_nodes.insert((pid, HIDPP20_DEFAULT_DEVICE_INDEX, serial)) {
                    devices.push(dev);
                }
                continue;
            }

            // 2. If this looks like a USB receiver on 0xFF00/2, probe wireless slots 1..=6.
            if (RECEIVER_PIDS.contains(&pid) || (usage_page == 0xFF00 && usage == 2))
                && dev_info.usage_page() == 0xFF00
            {
                for slot in 1u8..=6u8 {
                    let Ok(slot_handle) = api.open_path(dev_info.path()) else {
                        break;
                    };
                    if let Some(dev) = probe_hidpp20_slot(dev_info, &path, slot, slot_handle) {
                        let serial = dev_info.serial_number().unwrap_or_default().to_owned();
                        if seen_nodes.insert((pid, slot, serial)) {
                            devices.push(dev);
                        }
                    }
                }
                continue;
            }

            // 3. Fallback to known legacy Logitech protocols when HID++ 2.0 IRoot did not answer.
            if let Some(legacy_kind) = match_legacy_pid(pid, usage_page, usage)
                && let Ok(legacy_handle) = api.open_path(dev_info.path())
            {
                let product_name = dev_info
                    .product_string()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Logitech Legacy RGB Device")
                    .to_owned();
                let kind = infer_device_kind(&product_name, legacy_kind.default_kind());
                let info = DeviceInfo {
                    vendor: Vendor::Logitech,
                    name: product_name,
                    kind,
                    vid: LOGITECH_VID,
                    pid,
                    path: path.clone(),
                    protocol: legacy_kind.protocol_label().to_owned(),
                };
                let serial = dev_info.serial_number().unwrap_or_default().to_owned();
                if seen_nodes.insert((pid, HIDPP20_DEFAULT_DEVICE_INDEX, serial)) {
                    devices.push(Box::new(LegacyLogitechDevice {
                        handle: legacy_handle,
                        variant: legacy_kind,
                        info,
                    }));
                }
            }
        }

        Ok(devices)
    }
}

fn is_logitech_control_node(dev_info: &HidDeviceInfo) -> bool {
    let up = dev_info.usage_page();
    let u = dev_info.usage();
    let iface = dev_info.interface_number();

    // Ported from LogitechControllerDetect.cpp Unified HID++ 2.0 & legacy detectors:
    (up == 0xFF00 && u == 2)
        || up == 0xFF43
        || (iface == 1 && up == 0xFF00)
        || (up == 0xFFA0 && u == 1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RgbPageKind {
    RgbEffects8071,
    Centurion0600,
    ColorLed8070,
}

#[derive(Debug, Clone)]
struct ZoneEffectTarget {
    cluster_idx: u8,
    static_effect_idx: Option<u8>,
}

struct Hidpp20Capabilities {
    idx_rgb_effects: u8,
    rgb_page: Option<RgbPageKind>,
    idx_headset_hostmode: u8,
    idx_perkey_v1: u8,
    idx_perkey_v2: u8,
    zones: Vec<ZoneEffectTarget>,
}

fn probe_hidpp20_slot(
    dev_info: &HidDeviceInfo,
    path: &str,
    device_index: u8,
    handle: HidDevice,
) -> Option<Box<dyn BacklightDevice>> {
    // Confirm HID++ 2.0 via IRoot -> FeatureSet (0x0001).
    let timeout = if device_index == HIDPP20_DEFAULT_DEVICE_INDEX {
        45
    } else {
        25
    };
    let idx_feature_set =
        hidpp20_get_feature_index(&handle, device_index, FEAT_FEATURE_SET, timeout)?;
    if idx_feature_set == 0 {
        return None;
    }

    let (idx_rgb_effects, rgb_page) =
        if let Some(idx) = hidpp20_get_feature_index(&handle, device_index, FEAT_RGB_EFFECTS, 80) {
            if idx != 0 {
                (idx, Some(RgbPageKind::RgbEffects8071))
            } else {
                probe_secondary_rgb_page(&handle, device_index)
            }
        } else {
            probe_secondary_rgb_page(&handle, device_index)
        };

    let idx_headset_hostmode =
        hidpp20_get_feature_index(&handle, device_index, FEAT_HEADSET_RGB_HOSTMODE, 60)
            .unwrap_or(0);
    let idx_perkey_v2 =
        hidpp20_get_feature_index(&handle, device_index, FEAT_PER_KEY_V2, 60).unwrap_or(0);
    let idx_perkey_v1 =
        hidpp20_get_feature_index(&handle, device_index, FEAT_PER_KEY_V1, 60).unwrap_or(0);

    if idx_rgb_effects == 0 && idx_headset_hostmode == 0 && idx_perkey_v1 == 0 && idx_perkey_v2 == 0
    {
        // HID++ 2.0 non-illuminated mouse/keyboard (e.g., office peripherals): skip cleanly.
        return None;
    }

    let zones = if idx_rgb_effects != 0 {
        if let Some(page) = rgb_page {
            discover_zones(&handle, device_index, idx_rgb_effects, page)
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let queried_name = query_device_name(&handle, device_index);
    let usb_name = dev_info
        .product_string()
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let name = queried_name
        .or(usb_name)
        .unwrap_or_else(|| format!("Logitech HID++ Device ({:04X})", dev_info.product_id()));

    let inferred = infer_device_kind(&name, DeviceKind::Unknown);
    let protocol = match rgb_page {
        Some(RgbPageKind::RgbEffects8071) => format!("HID++ 2.0 (0x8071, idx={device_index:#04X})"),
        Some(RgbPageKind::Centurion0600) => format!("HID++ 2.0 (0x0600, idx={device_index:#04X})"),
        Some(RgbPageKind::ColorLed8070) => format!("HID++ 2.0 (0x8070, idx={device_index:#04X})"),
        None if idx_headset_hostmode != 0 => {
            format!("HID++ 2.0 Headset (0x0620, idx={device_index:#04X})")
        }
        None => format!("HID++ 2.0 PerKey (idx={device_index:#04X})"),
    };

    let info = DeviceInfo {
        vendor: Vendor::Logitech,
        name,
        kind: inferred,
        vid: LOGITECH_VID,
        pid: dev_info.product_id(),
        path: path.to_owned(),
        protocol,
    };

    Some(Box::new(Hidpp20BacklightDevice {
        handle,
        device_index,
        caps: Hidpp20Capabilities {
            idx_rgb_effects,
            rgb_page,
            idx_headset_hostmode,
            idx_perkey_v1,
            idx_perkey_v2,
            zones,
        },
        claimed_sw_control: false,
        info,
    }))
}

fn probe_secondary_rgb_page(handle: &HidDevice, device_index: u8) -> (u8, Option<RgbPageKind>) {
    if let Some(idx) = hidpp20_get_feature_index(handle, device_index, FEAT_CENTURION_RGB, 60)
        && idx != 0
    {
        return (idx, Some(RgbPageKind::Centurion0600));
    }
    if let Some(idx) = hidpp20_get_feature_index(handle, device_index, FEAT_COLOR_LED_EFFECTS, 60)
        && idx != 0
    {
        return (idx, Some(RgbPageKind::ColorLed8070));
    }
    (0, None)
}

fn query_device_name(handle: &HidDevice, device_index: u8) -> Option<String> {
    let idx = hidpp20_get_feature_index(handle, device_index, FEAT_DEVICE_NAME, 60)?;
    if idx == 0 {
        return None;
    }
    // fn0 GetDeviceNameCount()
    let resp = hidpp20_send_acked(handle, device_index, idx, 0x00, &[], 80).ok()??;
    let total_len = usize::from(*resp.first()?);
    if total_len == 0 || total_len > 64 {
        return None;
    }
    let mut name_bytes = Vec::with_capacity(total_len);
    let mut offset = 0u8;
    while name_bytes.len() < total_len {
        let chunk = hidpp20_send_acked(handle, device_index, idx, 0x10, &[offset], 80).ok()??;
        if chunk.is_empty() {
            break;
        }
        for &b in chunk.iter() {
            if name_bytes.len() >= total_len || b == 0 {
                break;
            }
            name_bytes.push(b);
        }
        let added = u8::try_from(chunk.len()).ok()?;
        offset = offset.saturating_add(added);
    }
    let text = String::from_utf8(name_bytes).ok()?;
    let trimmed = text.trim().to_owned();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn discover_zones(
    handle: &HidDevice,
    device_index: u8,
    idx_rgb: u8,
    page: RgbPageKind,
) -> Vec<ZoneEffectTarget> {
    let mut targets = Vec::new();
    let get_info_payload: &[u8] = match page {
        RgbPageKind::RgbEffects8071 => &[0xFF, 0xFF, 0x00],
        RgbPageKind::Centurion0600 | RgbPageKind::ColorLed8070 => &[],
    };

    let Ok(Some(info_resp)) =
        hidpp20_send_acked(handle, device_index, idx_rgb, 0x00, get_info_payload, 100)
    else {
        return targets;
    };

    let cluster_count = match page {
        RgbPageKind::RgbEffects8071 => info_resp.get(2).copied().unwrap_or(0),
        RgbPageKind::Centurion0600 | RgbPageKind::ColorLed8070 => {
            info_resp.first().copied().unwrap_or(0)
        }
    };

    for cluster_idx in 0..cluster_count.min(16) {
        let cluster_query = match page {
            RgbPageKind::RgbEffects8071 => [cluster_idx, 0xFF],
            RgbPageKind::Centurion0600 | RgbPageKind::ColorLed8070 => [cluster_idx, 0x00],
        };
        let fn_cluster = match page {
            RgbPageKind::RgbEffects8071 | RgbPageKind::Centurion0600 => 0x00,
            RgbPageKind::ColorLed8070 => 0x10,
        };
        let Ok(Some(c_resp)) = hidpp20_send_acked(
            handle,
            device_index,
            idx_rgb,
            fn_cluster,
            &cluster_query,
            80,
        ) else {
            continue;
        };

        let effect_count = match page {
            RgbPageKind::RgbEffects8071 => c_resp.get(4).copied().unwrap_or(0),
            RgbPageKind::Centurion0600 | RgbPageKind::ColorLed8070 => {
                c_resp.get(3).copied().unwrap_or(0)
            }
        };

        let fn_eff = match page {
            RgbPageKind::RgbEffects8071 | RgbPageKind::Centurion0600 => 0x00,
            RgbPageKind::ColorLed8070 => 0x20,
        };

        let mut static_effect_idx = None;
        for eff_idx in 0..effect_count.min(32) {
            let Ok(Some(e_resp)) = hidpp20_send_acked(
                handle,
                device_index,
                idx_rgb,
                fn_eff,
                &[cluster_idx, eff_idx, 0x00, 0x00],
                80,
            ) else {
                continue;
            };
            let eff_hi = u16::from(e_resp.get(2).copied().unwrap_or(0));
            let eff_lo = u16::from(e_resp.get(3).copied().unwrap_or(0));
            let effect_id = (eff_hi << 8) | eff_lo;
            if effect_id == 0x0001 {
                static_effect_idx = Some(eff_idx);
                break;
            }
        }

        targets.push(ZoneEffectTarget {
            cluster_idx,
            static_effect_idx,
        });
    }

    targets
}

struct Hidpp20BacklightDevice {
    handle: HidDevice,
    device_index: u8,
    caps: Hidpp20Capabilities,
    claimed_sw_control: bool,
    info: DeviceInfo,
}

impl BacklightDevice for Hidpp20BacklightDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn set_backlight_off(&mut self) -> Result<(), GuardError> {
        // Path A: 0x8071 / 0x0600 RGBEffects supports hardware LED sleep power mode (0x03).
        if matches!(
            self.caps.rgb_page,
            Some(RgbPageKind::RgbEffects8071 | RgbPageKind::Centurion0600)
        ) {
            if let Ok(Some(_)) = hidpp20_send_acked(
                &self.handle,
                self.device_index,
                self.caps.idx_rgb_effects,
                0x80,
                &[0x01, 0x03],
                200,
            ) {
                return Ok(());
            }

            // Fallback: claim SW control and paint every zone black (0,0,0).
            self.set_sw_control(true)?;
            self.set_all_zones_static(0, 0, 0)?;
            return Ok(());
        }

        // Path B: 0x8070 ColorLedEffects uses SetSWControl(1,1) + Static(0,0,0) per zone.
        if self.caps.rgb_page == Some(RgbPageKind::ColorLed8070) {
            self.set_sw_control(true)?;
            self.set_all_zones_static(0, 0, 0)?;
            return Ok(());
        }

        // Path C: 0x0620 Headset host mode.
        if self.caps.idx_headset_hostmode != 0 {
            let _ = hidpp20_send_acked(
                &self.handle,
                self.device_index,
                self.caps.idx_headset_hostmode,
                0x80,
                &[0x01],
                150,
            )?;
            let mut single_cmd = [0u8; 16];
            write_u8(&mut single_cmd, 0, 0x00)?;
            write_u8(&mut single_cmd, 1, 0x00)?;
            write_u8(&mut single_cmd, 2, 0x00)?;
            write_u8(&mut single_cmd, 3, 0xFF)?;
            let _ = hidpp20_send_acked(
                &self.handle,
                self.device_index,
                self.caps.idx_headset_hostmode,
                0x50,
                &single_cmd,
                150,
            )?;
            let _ = hidpp20_send_acked(
                &self.handle,
                self.device_index,
                self.caps.idx_headset_hostmode,
                0x60,
                &[0x01],
                150,
            )?;
            return Ok(());
        }

        // Path D: Pure per-key 0x8080/0x8081 fallback (black frame commit).
        if self.caps.idx_perkey_v2 != 0 || self.caps.idx_perkey_v1 != 0 {
            return self.set_perkey_off_best_effort();
        }

        Err(GuardError::Protocol(
            "no controllable Logitech RGB feature found",
        ))
    }

    fn set_backlight_on(&mut self, restore: RestoreMode) -> Result<(), GuardError> {
        match restore {
            RestoreMode::Native => self.restore_native(),
            RestoreMode::Static { r, g, b } => self.restore_static(r, g, b),
        }
    }
}

impl Hidpp20BacklightDevice {
    fn set_sw_control(&mut self, enable: bool) -> Result<(), GuardError> {
        let Some(page) = self.caps.rgb_page else {
            return Ok(());
        };
        let (fn_sw, payload): (u8, [u8; 3]) = match page {
            RgbPageKind::ColorLed8070 => {
                let en = u8::from(enable);
                (0x80, [en, en, 0x00])
            }
            RgbPageKind::RgbEffects8071 | RgbPageKind::Centurion0600 => {
                if enable {
                    (0x50, [0x01, 0x03, 0x06])
                } else {
                    (0x50, [0x01, 0x00, 0x00])
                }
            }
        };

        let send_len = if page == RgbPageKind::ColorLed8070 {
            2
        } else {
            3
        };
        let data = payload
            .get(..send_len)
            .ok_or(GuardError::Protocol("invalid SW control slice"))?;
        let _ = hidpp20_send_acked(
            &self.handle,
            self.device_index,
            self.caps.idx_rgb_effects,
            fn_sw,
            data,
            200,
        )?;
        self.claimed_sw_control = enable;
        Ok(())
    }

    fn set_all_zones_static(&self, r: u8, g: u8, b: u8) -> Result<(), GuardError> {
        let Some(page) = self.caps.rgb_page else {
            return Ok(());
        };
        let fn_set_effect = match page {
            RgbPageKind::ColorLed8070 => 0x30,
            RgbPageKind::RgbEffects8071 | RgbPageKind::Centurion0600 => 0x10,
        };

        if self.caps.zones.is_empty() {
            // Try cluster 0, effect_idx 1 (typical Static index) / 0 as best-effort.
            let mut data = [0u8; 16];
            write_u8(&mut data, 0, 0)?;
            write_u8(&mut data, 1, 1)?;
            write_u8(&mut data, 2, r)?;
            write_u8(&mut data, 3, g)?;
            write_u8(&mut data, 4, b)?;
            if r != 0 || g != 0 || b != 0 {
                write_u8(&mut data, 5, 0x02)?;
            }
            let _ = hidpp20_send_acked(
                &self.handle,
                self.device_index,
                self.caps.idx_rgb_effects,
                fn_set_effect,
                &data,
                200,
            );
            return Ok(());
        }

        for zone in &self.caps.zones {
            let eff_idx = zone.static_effect_idx.unwrap_or(1);
            let mut data = [0u8; 16];
            write_u8(&mut data, 0, zone.cluster_idx)?;
            write_u8(&mut data, 1, eff_idx)?;
            write_u8(&mut data, 2, r)?;
            write_u8(&mut data, 3, g)?;
            write_u8(&mut data, 4, b)?;
            if r != 0 || g != 0 || b != 0 {
                write_u8(&mut data, 5, 0x02)?;
            }
            let _ = hidpp20_send_acked(
                &self.handle,
                self.device_index,
                self.caps.idx_rgb_effects,
                fn_set_effect,
                &data,
                200,
            );
        }
        Ok(())
    }

    fn set_perkey_off_best_effort(&self) -> Result<(), GuardError> {
        if self.caps.idx_perkey_v2 != 0 {
            // 0x8081 fn6 SetRgbZonesSingleValue [r, g, b, range_start, range_end]
            let _ = hidpp20_send_acked(
                &self.handle,
                self.device_index,
                self.caps.idx_perkey_v2,
                0x60,
                &[0x00, 0x00, 0x00, 0x00, 0xFF],
                150,
            )?;
            let _ = hidpp20_send_acked(
                &self.handle,
                self.device_index,
                self.caps.idx_perkey_v2,
                0x80,
                &[0x01],
                150,
            )?;
        }
        Ok(())
    }

    fn restore_native(&mut self) -> Result<(), GuardError> {
        if matches!(
            self.caps.rgb_page,
            Some(RgbPageKind::RgbEffects8071 | RgbPageKind::Centurion0600)
        ) {
            let _ = hidpp20_send_acked(
                &self.handle,
                self.device_index,
                self.caps.idx_rgb_effects,
                0x80,
                &[0x01, 0x01],
                200,
            );
            if self.claimed_sw_control {
                let _ = self.set_sw_control(false);
            }
            return Ok(());
        }

        if self.caps.rgb_page == Some(RgbPageKind::ColorLed8070) {
            // Release SW control so onboard/G-Hub lighting returns immediately.
            return self.set_sw_control(false);
        }

        if self.caps.idx_headset_hostmode != 0 {
            let _ = hidpp20_send_acked(
                &self.handle,
                self.device_index,
                self.caps.idx_headset_hostmode,
                0x80,
                &[0x00],
                150,
            )?;
            return Ok(());
        }

        Ok(())
    }

    fn restore_static(&mut self, r: u8, g: u8, b: u8) -> Result<(), GuardError> {
        if matches!(
            self.caps.rgb_page,
            Some(RgbPageKind::RgbEffects8071 | RgbPageKind::Centurion0600)
        ) {
            let _ = hidpp20_send_acked(
                &self.handle,
                self.device_index,
                self.caps.idx_rgb_effects,
                0x80,
                &[0x01, 0x01],
                200,
            );
            self.set_sw_control(true)?;
            self.set_all_zones_static(r, g, b)?;
            return Ok(());
        }

        if self.caps.rgb_page == Some(RgbPageKind::ColorLed8070) {
            self.set_sw_control(true)?;
            self.set_all_zones_static(r, g, b)?;
            return Ok(());
        }

        if self.caps.idx_headset_hostmode != 0 {
            let _ = hidpp20_send_acked(
                &self.handle,
                self.device_index,
                self.caps.idx_headset_hostmode,
                0x80,
                &[0x01],
                150,
            )?;
            let mut single_cmd = [0u8; 16];
            write_u8(&mut single_cmd, 0, r)?;
            write_u8(&mut single_cmd, 1, g)?;
            write_u8(&mut single_cmd, 2, b)?;
            write_u8(&mut single_cmd, 3, 0xFF)?;
            let _ = hidpp20_send_acked(
                &self.handle,
                self.device_index,
                self.caps.idx_headset_hostmode,
                0x50,
                &single_cmd,
                150,
            )?;
            let _ = hidpp20_send_acked(
                &self.handle,
                self.device_index,
                self.caps.idx_headset_hostmode,
                0x60,
                &[0x01],
                150,
            )?;
            return Ok(());
        }

        Ok(())
    }
}

/// Builds a 20-byte HID++ 2.0 long report packet (`0x11`).
pub fn build_hidpp20_long_frame(
    device_index: u8,
    feat_idx: u8,
    function: u8,
    payload: &[u8],
) -> Result<[u8; HIDPP_LONG_LEN], GuardError> {
    let mut buf = [0u8; HIDPP_LONG_LEN];
    write_u8(&mut buf, 0, HIDPP_LONG_REPORT_ID)?;
    write_u8(&mut buf, 1, device_index)?;
    write_u8(&mut buf, 2, feat_idx)?;
    write_u8(&mut buf, 3, (function & 0xF0) | HIDPP20_SW_ID)?;

    let max_copy = HIDPP_LONG_LEN.saturating_sub(4);
    let copy_len = payload.len().min(max_copy);
    if let Some(src) = payload.get(..copy_len) {
        copy_into_slice(&mut buf, 4, src)?;
    }
    Ok(buf)
}

fn hidpp20_get_feature_index(
    handle: &HidDevice,
    device_index: u8,
    feature_page: u16,
    timeout_ms: u64,
) -> Option<u8> {
    let hi = ((feature_page >> 8) & 0xFF) as u8;
    let lo = (feature_page & 0xFF) as u8;
    match hidpp20_send_acked(handle, device_index, 0x00, 0x00, &[hi, lo], timeout_ms) {
        Ok(Some(resp)) => resp.first().copied(),
        _ => None,
    }
}

fn hidpp20_send_acked(
    handle: &HidDevice,
    device_index: u8,
    feat_idx: u8,
    function: u8,
    payload: &[u8],
    timeout_ms: u64,
) -> Result<Option<Vec<u8>>, GuardError> {
    let frame = build_hidpp20_long_frame(device_index, feat_idx, function, payload)?;

    for attempt in 0u8..3u8 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(20));
        }

        handle.write(&frame)?;

        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut read_buf = [0u8; HIDPP_MAX_READ_LEN];
        let mut busy_retry = false;

        for _ in 0..32u8 {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let rem_ms = deadline.saturating_duration_since(now).as_millis();
            let step_timeout = i32::try_from(rem_ms.min(50)).unwrap_or(50).max(1);

            let rd = match handle.read_timeout(&mut read_buf, step_timeout) {
                Ok(n) if n > 0 => n,
                Ok(_) => break,
                Err(_) => break,
            };

            let r_report = read_buf.first().copied().unwrap_or(0);
            if r_report != HIDPP_SHORT_REPORT_ID
                && r_report != HIDPP_LONG_REPORT_ID
                && r_report != HIDPP_VERY_LONG_REPORT_ID
            {
                continue;
            }

            if read_buf.get(1).copied() != Some(device_index) {
                continue;
            }

            let resp_feat = read_buf.get(2).copied().unwrap_or(0);
            let resp_func = read_buf.get(3).copied().unwrap_or(0);

            if resp_feat == HIDPP20_ERROR_FEAT {
                let err_feat = resp_func;
                let err_func = read_buf.get(4).copied().unwrap_or(0);
                let err_code = read_buf.get(5).copied().unwrap_or(0);

                if err_feat == feat_idx
                    && (err_func & 0xF0) == (function & 0xF0)
                    && (err_func & 0x0F) == HIDPP20_SW_ID
                {
                    if err_code == HIDPP20_ERR_BUSY {
                        busy_retry = true;
                        break;
                    }
                    return Ok(None);
                }
                continue;
            }

            if resp_feat == feat_idx
                && (resp_func & 0xF0) == (function & 0xF0)
                && (resp_func & 0x0F) == HIDPP20_SW_ID
            {
                let payload_slice = read_buf.get(4..rd).unwrap_or(&[]);
                return Ok(Some(payload_slice.to_vec()));
            }
        }

        if !busy_retry && attempt == 0 {
            // Allow one extra quick attempt for wake-from-idle latency on wireless devices.
            continue;
        }
    }

    Ok(None)
}

#[derive(Debug, Clone, Copy)]
enum LegacyVariant {
    G213,
    G815,
    G203Prodigy,
    G203Lightsync,
}

impl LegacyVariant {
    fn default_kind(self) -> DeviceKind {
        match self {
            Self::G213 | Self::G815 => DeviceKind::Keyboard,
            Self::G203Prodigy | Self::G203Lightsync => DeviceKind::Mouse,
        }
    }

    fn protocol_label(self) -> &'static str {
        match self {
            Self::G213 => "Logitech G213 Legacy",
            Self::G815 => "Logitech G815 Legacy",
            Self::G203Prodigy => "Logitech G203 Legacy",
            Self::G203Lightsync => "Logitech Lightsync Legacy",
        }
    }
}

fn match_legacy_pid(pid: u16, usage_page: u16, usage: u16) -> Option<LegacyVariant> {
    match pid {
        0xC336 if usage_page == 0xFF43 => Some(LegacyVariant::G213),
        0xC33E | 0xC33F if usage_page == 0xFF43 => Some(LegacyVariant::G815),
        0xC084 if usage_page == 0xFF00 => Some(LegacyVariant::G203Prodigy),
        0xC092 | 0xC09D if usage_page == 0xFF00 && usage == 2 => Some(LegacyVariant::G203Lightsync),
        _ => None,
    }
}

struct LegacyLogitechDevice {
    handle: HidDevice,
    variant: LegacyVariant,
    info: DeviceInfo,
}

impl BacklightDevice for LegacyLogitechDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn set_backlight_off(&mut self) -> Result<(), GuardError> {
        let pkt = match self.variant {
            LegacyVariant::G213 => build_legacy_mode_packet(0x0C, 0x3C, 0x00, 0x00, [0, 0, 0], 0)?,
            LegacyVariant::G815 => build_legacy_mode_packet(0x0D, 0x3D, 0x00, 0x00, [0, 0, 0], 0)?,
            LegacyVariant::G203Prodigy => {
                build_legacy_mode_packet(0x0E, 0x3A, 0x00, 0x00, [0, 0, 0], 0)?
            }
            LegacyVariant::G203Lightsync => {
                build_legacy_mode_packet(0x0E, 0x1E, 0x00, 0x00, [0, 0, 0], 0)?
            }
        };
        self.handle.write(&pkt)?;
        if matches!(self.variant, LegacyVariant::G815) {
            let commit = build_legacy_g815_commit()?;
            self.handle.write(&commit)?;
        }
        Ok(())
    }

    fn set_backlight_on(&mut self, restore: RestoreMode) -> Result<(), GuardError> {
        let rgb = match restore {
            RestoreMode::Native => [255, 255, 255],
            RestoreMode::Static { r, g, b } => [r, g, b],
        };
        let pkt = match self.variant {
            LegacyVariant::G213 => build_legacy_mode_packet(0x0C, 0x3C, 0x00, 0x01, rgb, 0x02)?,
            LegacyVariant::G815 => build_legacy_mode_packet(0x0D, 0x3D, 0x00, 0x01, rgb, 0x00)?,
            LegacyVariant::G203Prodigy => {
                build_legacy_mode_packet(0x0E, 0x3A, 0x00, 0x01, rgb, 0x02)?
            }
            LegacyVariant::G203Lightsync => {
                build_legacy_mode_packet(0x0E, 0x1E, 0x00, 0x01, rgb, 0x02)?
            }
        };
        self.handle.write(&pkt)?;
        if matches!(self.variant, LegacyVariant::G815) {
            let commit = build_legacy_g815_commit()?;
            self.handle.write(&commit)?;
        }
        Ok(())
    }
}

/// Builds a 20-byte legacy Logitech G-series/Lightsync mode packet.
pub fn build_legacy_mode_packet(
    feat_idx: u8,
    cmd: u8,
    zone: u8,
    mode: u8,
    rgb: [u8; 3],
    tail: u8,
) -> Result<[u8; HIDPP_LONG_LEN], GuardError> {
    let mut buf = [0u8; HIDPP_LONG_LEN];
    write_u8(&mut buf, 0, 0x11)?;
    write_u8(&mut buf, 1, 0xFF)?;
    write_u8(&mut buf, 2, feat_idx)?;
    write_u8(&mut buf, 3, cmd)?;
    write_u8(&mut buf, 4, zone)?;
    write_u8(&mut buf, 5, mode)?;
    write_u8(&mut buf, 6, rgb[0])?;
    write_u8(&mut buf, 7, rgb[1])?;
    write_u8(&mut buf, 8, rgb[2])?;
    write_u8(&mut buf, 9, tail)?;
    Ok(buf)
}

fn build_legacy_g815_commit() -> Result<[u8; HIDPP_LONG_LEN], GuardError> {
    let mut buf = [0u8; HIDPP_LONG_LEN];
    write_u8(&mut buf, 0, 0x11)?;
    write_u8(&mut buf, 1, 0xFF)?;
    write_u8(&mut buf, 2, 0x10)?;
    write_u8(&mut buf, 3, 0x7F)?;
    Ok(buf)
}

fn infer_device_kind(name: &str, fallback: DeviceKind) -> DeviceKind {
    let lower = name.to_ascii_lowercase();
    if lower.contains("mouse")
        || lower.contains("g502")
        || lower.contains("g203")
        || lower.contains("g303")
        || lower.contains("g403")
        || lower.contains("g703")
        || lower.contains("g900")
        || lower.contains("g903")
        || lower.contains("superlight")
    {
        DeviceKind::Mouse
    } else if lower.contains("keyboard")
        || lower.contains("g213")
        || lower.contains("g413")
        || lower.contains("g512")
        || lower.contains("g515")
        || lower.contains("g610")
        || lower.contains("g810")
        || lower.contains("g813")
        || lower.contains("g815")
        || lower.contains("g910")
        || lower.contains("g915")
        || lower.contains("pro x")
    {
        DeviceKind::Keyboard
    } else if lower.contains("headset")
        || lower.contains("g522")
        || lower.contains("g633")
        || lower.contains("g635")
        || lower.contains("g733")
        || lower.contains("g933")
        || lower.contains("g935")
    {
        DeviceKind::Headset
    } else if lower.contains("powerplay") || lower.contains("mat") {
        DeviceKind::Mousemat
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidpp20_long_frame_layout_matches_openrgb() {
        let frame = build_hidpp20_long_frame(0xFF, 0x00, 0x00, &[0x80, 0x71]).unwrap();
        assert_eq!(frame.len(), 20);
        assert_eq!(frame[0], 0x11);
        assert_eq!(frame[1], 0xFF);
        assert_eq!(frame[2], 0x00);
        assert_eq!(frame[3], 0x07);
        assert_eq!(frame[4], 0x80);
        assert_eq!(frame[5], 0x71);
        assert_eq!(frame[6], 0x00);
    }

    #[test]
    fn hidpp20_sw_id_masks_upper_function_nibble() {
        let frame = build_hidpp20_long_frame(0x01, 0x09, 0x80, &[0x01, 0x03]).unwrap();
        assert_eq!(frame[3], 0x87);
        assert_eq!(frame[4], 0x01);
        assert_eq!(frame[5], 0x03);
    }

    #[test]
    fn legacy_g213_packet_matches_openrgb() {
        let pkt = build_legacy_mode_packet(0x0C, 0x3C, 0x00, 0x01, [10, 20, 30], 0x02).unwrap();
        assert_eq!(pkt[0], 0x11);
        assert_eq!(pkt[1], 0xFF);
        assert_eq!(pkt[2], 0x0C);
        assert_eq!(pkt[3], 0x3C);
        assert_eq!(pkt[4], 0x00);
        assert_eq!(pkt[5], 0x01);
        assert_eq!(pkt[6], 10);
        assert_eq!(pkt[7], 20);
        assert_eq!(pkt[8], 30);
        assert_eq!(pkt[9], 0x02);
    }
}
