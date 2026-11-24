# hid-backlight-guard

[![CI](https://github.com/drlalx/hid-backlight-guard/actions/workflows/ci.yml/badge.svg)](https://github.com/drlalx/hid-backlight-guard/actions/workflows/ci.yml)
[![License: GPL v2+](https://img.shields.io/badge/License-GPL--2.0--or--later-blue.svg)](LICENSE)

> [Deutsche Version (README.de.md)](README.de.md)

A lightweight, **cross-platform pure-Rust** utility (**Windows, Linux, macOS**) that automatically turns off peripheral backlights (keyboards, mice, headsets, mousemats, etc.) when the display turns off or the system enters sleep mode, and restores them on wake.

## Features

- **Cross-Platform Native Integration**:
  - **Windows**: Message-only `WM_POWERBROADCAST` listener + HKCU `Run` autostart + static MSVC CRT (`+crt-static`, no Visual C++ Redistributable required).
  - **Linux**: Pure-Rust `hidraw` (`linux-native-basic-udev`, zero `libudev` C dependency), D-Bus listener (`systemd-logind` `PrepareForSleep` + `ScreenSaver` `ActiveChanged` for KDE/GNOME/XFCE/Cinnamon/MATE), and `systemd --user` / XDG autostart.
  - **macOS**: Native Apple `IOKit` listener (`IORegisterForSystemPower` + `IODisplayWrangler`) and `LaunchAgent` autostart.
- **100% Pure Rust, Zero-Panic Production Code**: Enforced via `[lints.clippy]` in `Cargo.toml` (`unwrap_used`, `expect_used`, `panic`, `panic_in_result_fn`, `todo`, `unimplemented`, `unreachable`, `indexing_slicing`, `undocumented_unsafe_blocks`, `dbg_macro` all set to `deny`); panics are permitted only inside `#[cfg(test)]` unit tests.
- **Vendor-Agnostic & Extensible**: Modular trait-based backend design (`BacklightBackend` / `BacklightDevice` in `src/device.rs`).
- **Preserves Your Existing Lighting by Default**: On wake (`Native` restore mode), brightness / hardware power management is re-enabled without forcing flashy RGB presets. For Razer Extended devices, firmware regions (`0x0F:0x80`) are queried dynamically and an active effect (`0x0F:0x82`) is restored automatically if a device was stuck in `OFF` (`0x00`). Optionally use `--static R,G,B` to apply a solid color instead.

## Installation

### Prebuilt Binaries

Download the archive for your OS (`x86_64-unknown-linux-musl`, `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc`, or `aarch64-pc-windows-msvc`) together with its `.sha256` checksum from the [GitHub Releases](https://github.com/drlalx/hid-backlight-guard/releases) page.

- **Linux (non-root HID access):** Install the bundled [60-hid-backlight-guard.rules](packaging/60-hid-backlight-guard.rules):
  ```bash
  sudo cp 60-hid-backlight-guard.rules /etc/udev/rules.d/
  sudo udevadm control --reload-rules && sudo udevadm trigger
  ```
- **macOS:** Grant **Input Monitoring** permission to your terminal / `hid-backlight-guard` in *System Settings → Privacy & Security → Input Monitoring*.

### Build from Source

Requires Rust `1.88+` (Edition 2024):

```bash
git clone https://github.com/drlalx/hid-backlight-guard.git
cd hid-backlight-guard
cargo build --release
```

## Supported Vendors (OpenRGB Protocol Port)

- **Razer (`src/backends/razer.rs`, `src/backends/razer_devices.rs`)**:
  - 187 ported device/interface descriptors from OpenRGB (`Standard`, `Linear`, `Extended`, `ExtendedArgb`, `Custom`) covering keyboards, mice, keypads, headsets, mousemats, and accessories, plus a generic Razer HID interface fallback.
  - Dynamically queries hardware LED regions (`0x0F:0x80`) on Extended devices (for example Scroll Wheel, Logo, and Numpad zones on multi-zone mice).
- **Logitech (`src/backends/logitech.rs`)**:
  - Unified **HID++ 2.0** (`0x8071` RGB Effects power management, `0x8070` Color LED Effects, `0x0620` Headset Hostmode, `0x8081` Per-Key) for direct USB/Bluetooth devices as well as wireless receiver slots (`1..=6`), plus legacy protocol fallbacks (G213, G813/G815, G203).

## Usage

```powershell
# List detected compatible HID devices
hid-backlight-guard.exe --list

# Query hardware lighting regions, brightness, and active effect state
hid-backlight-guard.exe --probe

# Turn peripheral backlights off / on manually
hid-backlight-guard.exe --off
hid-backlight-guard.exe --on
hid-backlight-guard.exe --on --static 255,255,255

# Start background display/power monitor (default when no flags are given)
hid-backlight-guard.exe --background
hid-backlight-guard.exe --background --silent

# Manage Windows autostart in HKCU\Software\Microsoft\Windows\CurrentVersion\Run
hid-backlight-guard.exe --install-autostart
hid-backlight-guard.exe --uninstall-autostart
```

## Development & Tests

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release
```

### Adding Another Vendor Backend

1. Create a new module at `src/backends/<vendor>.rs`.
2. Implement `BacklightBackend` (device discovery via `hidapi::HidApi`) and `BacklightDevice` (`set_backlight_off` / `set_backlight_on`).
3. Register the backend in `default_backends()` inside `src/backends/mod.rs`.

## License

Licensed under the [GNU General Public License v2.0 or later](LICENSE) (`GPL-2.0-or-later`).
Protocol definitions and device descriptor tables are ported from [OpenRGB](https://gitlab.com/CalcProgrammer1/OpenRGB).
