//! Vendor-independent backlight control abstractions and error types.

use std::fmt;

use hidapi::HidApi;

/// Peripheral manufacturer identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    /// Razer Inc. (`VID 0x1532`).
    Razer,
    /// Logitech (`VID 0x046D`).
    Logitech,
    /// Any other vendor identified by name.
    Other(&'static str),
}

impl fmt::Display for Vendor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Razer => f.write_str("Razer"),
            Self::Logitech => f.write_str("Logitech"),
            Self::Other(name) => f.write_str(name),
        }
    }
}

/// Hardware form factor of a discovered HID peripheral.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    /// Full or compact keyboard.
    Keyboard,
    /// Wired or wireless mouse.
    Mouse,
    /// One-handed gaming keypad.
    Keypad,
    /// Audio headset or headphones.
    Headset,
    /// Illuminated mousemat or desk mat.
    Mousemat,
    /// Lighting controller, dock, stand, or other accessory.
    Accessory,
    /// Unclassified peripheral type.
    Unknown,
}

impl fmt::Display for DeviceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Keyboard => "Keyboard",
            Self::Mouse => "Mouse",
            Self::Keypad => "Keypad",
            Self::Headset => "Headset",
            Self::Mousemat => "Mousemat",
            Self::Accessory => "Accessory",
            Self::Unknown => "Unknown",
        };
        f.write_str(label)
    }
}

/// Strategy used when restoring a peripheral's backlight on display/system wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RestoreMode {
    /// Restore brightness/power without overriding the user's configured lighting effect or color.
    #[default]
    Native,
    /// Set a solid static color after waking the backlight.
    Static {
        /// Red channel (`0..=255`).
        r: u8,
        /// Green channel (`0..=255`).
        g: u8,
        /// Blue channel (`0..=255`).
        b: u8,
    },
}

/// Metadata describing one controllable backlight device.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Peripheral manufacturer.
    pub vendor: Vendor,
    /// Human-readable device name.
    pub name: String,
    /// Peripheral hardware form factor.
    pub kind: DeviceKind,
    /// USB Vendor ID.
    pub vid: u16,
    /// USB Product ID.
    pub pid: u16,
    /// OS-specific HID device path.
    pub path: String,
    /// Active lighting control protocol summary.
    pub protocol: String,
}

/// Unified error type for HID backlight discovery, protocol I/O, and Windows hooks.
#[derive(Debug)]
pub enum GuardError {
    /// Underlying `hidapi` error.
    Hid(hidapi::HidError),
    /// Win32 API call failure with operation name and error code.
    Win32 {
        /// Win32 function name.
        op: &'static str,
        /// Win32 error code returned by the API.
        code: u32,
    },
    /// Vendor HID protocol error or invalid packet framing.
    Protocol(&'static str),
    /// Invalid CLI argument or configuration parameter.
    InvalidArgument(&'static str),
    /// Standard I/O error.
    Io(std::io::Error),
}

impl fmt::Display for GuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hid(err) => write!(f, "HID error: {err}"),
            Self::Win32 { op, code } => write!(f, "Win32 {op} failed (error {code})"),
            Self::Protocol(msg) => write!(f, "Protocol error: {msg}"),
            Self::InvalidArgument(msg) => write!(f, "Invalid argument: {msg}"),
            Self::Io(err) => write!(f, "I/O error: {err}"),
        }
    }
}

impl std::error::Error for GuardError {}

impl From<hidapi::HidError> for GuardError {
    fn from(err: hidapi::HidError) -> Self {
        Self::Hid(err)
    }
}

impl From<std::io::Error> for GuardError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// A single open peripheral whose backlight can be turned off and restored.
pub trait BacklightDevice {
    /// Returns metadata identifying this device.
    fn info(&self) -> &DeviceInfo;
    /// Turns the device's backlight off.
    fn set_backlight_off(&mut self) -> Result<(), GuardError>;
    /// Turns the device's backlight back on using the requested restore strategy.
    fn set_backlight_on(&mut self, restore: RestoreMode) -> Result<(), GuardError>;
    /// Queries device-specific diagnostic details (LED zones, brightness, active effect).
    fn probe_details(&mut self) -> Result<Vec<String>, GuardError> {
        Ok(Vec::new())
    }
}

/// Vendor backend responsible for discovering matching peripherals via `HidApi`.
pub trait BacklightBackend {
    /// Short identifier for this backend (e.g., `"razer"`, `"logitech"`).
    fn name(&self) -> &'static str;
    /// Discovers all controllable devices handled by this backend.
    fn discover(&self, api: &HidApi) -> Result<Vec<Box<dyn BacklightDevice>>, GuardError>;
}

/// Writes `value` at `index` in `buf`, returning `GuardError::Protocol` on out-of-bounds access.
#[inline]
pub fn write_u8(buf: &mut [u8], index: usize, value: u8) -> Result<(), GuardError> {
    let slot = buf
        .get_mut(index)
        .ok_or(GuardError::Protocol("packet buffer index out of bounds"))?;
    *slot = value;
    Ok(())
}

/// Copies up to `dst_len` bytes from `src` into `dst[offset..]` without panicking.
#[inline]
pub fn copy_into_slice(dst: &mut [u8], offset: usize, src: &[u8]) -> Result<(), GuardError> {
    let end = offset
        .checked_add(src.len())
        .ok_or(GuardError::Protocol("packet offset overflow"))?;
    let target = dst
        .get_mut(offset..end)
        .ok_or(GuardError::Protocol("packet copy exceeds buffer bounds"))?;
    target.copy_from_slice(src);
    Ok(())
}
