//! Vendor-independent backlight control abstractions and error types.

use std::fmt;

use hidapi::HidApi;

/// Peripheral manufacturer identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Razer,
    Logitech,
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
    Keyboard,
    Mouse,
    Keypad,
    Headset,
    Mousemat,
    Accessory,
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
    Static { r: u8, g: u8, b: u8 },
}

/// Metadata describing one controllable backlight device.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub vendor: Vendor,
    pub name: String,
    pub kind: DeviceKind,
    pub vid: u16,
    pub pid: u16,
    pub path: String,
    pub protocol: String,
}

/// Unified error type for HID backlight discovery, protocol I/O, and Windows hooks.
#[derive(Debug)]
pub enum GuardError {
    Hid(hidapi::HidError),
    Win32 { op: &'static str, code: u32 },
    Protocol(&'static str),
    InvalidArgument(&'static str),
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
    fn info(&self) -> &DeviceInfo;
    fn set_backlight_off(&mut self) -> Result<(), GuardError>;
    fn set_backlight_on(&mut self, restore: RestoreMode) -> Result<(), GuardError>;
}

/// Vendor backend responsible for discovering matching peripherals via `HidApi`.
pub trait BacklightBackend {
    fn name(&self) -> &'static str;
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
