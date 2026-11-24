# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Full cross-platform support for **Linux** (pure-Rust `hidapi` via `linux-native-basic-udev`, D-Bus `systemd-logind` + `ScreenSaver` power/display monitor via `zbus`, `systemd --user` / XDG autostart, and `packaging/60-hid-backlight-guard.rules` udev rules) and **macOS** (native `IOKit`/`CoreFoundation` system & display power monitor, `LaunchAgent` autostart, shared HID device access).
- Multi-OS CI matrix (`ubuntu-latest`, `windows-latest`, `macos-latest`) and cross-platform release artifacts (`x86_64-unknown-linux-musl`, `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`).

## [0.1.0] - 2026-11-24

### Added
- Pure-Rust Windows display & power monitor (`WM_POWERBROADCAST`) that automatically turns off peripheral backlights on display off / suspend and restores them on wake.
- Razer HID backend with 187 OpenRGB device descriptors, dynamic Extended region discovery (`0x0F:0x80`), and automatic active-effect recovery when stuck in `OFF` (`0x00`).
- Logitech Unified HID++ 2.0 backend (`0x8071`, `0x8070`, `0x0620`, `0x8081`, wireless receiver slots `1..=6`) plus legacy G-series/Lightsync fallbacks.
- CLI flags `--list`, `--probe`, `--off`, `--on`, `--static R,G,B`, `--background`, `--silent`, `--install-autostart`, and `--uninstall-autostart`.
- GitHub release infrastructure: CI (`ci.yml`), automated Windows `x86_64` & `aarch64` releases (`release.yml`) with SHA-256 checksums, Dependabot, `deny.toml`, static CRT linking (`.cargo/config.toml`), and dual-layer license/security compliance tests.
