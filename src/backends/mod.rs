//! Manufacturer-specific HID backlight backends.

pub mod logitech;
pub mod razer;
#[allow(dead_code)]
pub mod razer_devices;

use hidapi::HidApi;

use crate::device::{BacklightBackend, BacklightDevice, GuardError};

/// Returns all enabled vendor backends in discovery order.
pub fn default_backends() -> Vec<Box<dyn BacklightBackend>> {
    vec![
        Box::new(razer::RazerBackend),
        Box::new(logitech::LogitechBackend),
    ]
}

/// Enumerates all currently attached backlight devices across all registered backends.
/// Discovery failures for individual backends or devices are logged to `stderr` rather than aborting other vendors.
pub fn discover_all_devices(api: &HidApi) -> Result<Vec<Box<dyn BacklightDevice>>, GuardError> {
    let mut devices = Vec::new();
    for backend in default_backends() {
        match backend.discover(api) {
            Ok(mut found) => devices.append(&mut found),
            Err(err) => {
                eprintln!("[{}] Discovery warning: {err}", backend.name());
            }
        }
    }
    Ok(devices)
}
