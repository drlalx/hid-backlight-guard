# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Pure-Rust Windows display & power monitor (`WM_POWERBROADCAST`) that automatically turns off peripheral backlights on display off / suspend and restores them on wake.
- Razer HID backend with 187 OpenRGB device descriptors, dynamic Extended region discovery (`0x0F:0x80`), and automatic active-effect recovery when stuck in `OFF` (`0x00`).
- Logitech Unified HID++ 2.0 backend (`0x8071`, `0x8070`, `0x0620`, `0x8081`, wireless receiver slots `1..=6`) plus legacy G-series/Lightsync fallbacks.
- CLI flags `--list`, `--probe`, `--off`, `--on`, `--static R,G,B`, `--background`, `--silent`, `--install-autostart`, and `--uninstall-autostart`.
- GitHub release infrastructure: CI (`ci.yml`), automated Windows `x86_64` & `aarch64` releases (`release.yml`) with SHA-256 checksums, Dependabot, `deny.toml`, static CRT linking (`.cargo/config.toml`), and dual-layer license/security compliance tests.
