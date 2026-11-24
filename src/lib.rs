//! Cross-platform pure-Rust daemon and library (Windows, Linux, macOS) for automatically
//! turning off HID peripheral backlights when the display turns off or the system suspends,
//! and restoring them on wake.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod autostart;
pub mod backends;
pub mod device;
pub mod power_monitor;
