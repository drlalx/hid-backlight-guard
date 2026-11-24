# hid-backlight-guard

[![CI](https://github.com/drlalx/hid-backlight-guard/actions/workflows/ci.yml/badge.svg)](https://github.com/drlalx/hid-backlight-guard/actions/workflows/ci.yml)
[![License: GPL v2+](https://img.shields.io/badge/License-GPL--2.0--or--later-blue.svg)](LICENSE)

> [English version (README.md)](README.md)

Ein schlankes, komplett in **pure Rust** geschriebenes Windows-Dienstprogramm, das die Hintergrundbeleuchtung unterstützter HID-Peripheriegeräte (Tastaturen, Mäuse, Headsets, Mousepads usw.) automatisch ausschaltet, sobald der Bildschirm abgeschaltet wird oder der PC in den Energiesparmodus wechselt – und beim Aufwecken wiederherstellt.

## Merkmale

- **Pure Rust, Zero-Panic in Produktivcode**: Keine `.unwrap()`, `.expect()`, `panic!()` oder ungeprüften Index-Zugriffe im Produktivcode (zentral über `[lints.clippy]` in `Cargo.toml` erzwungen; Panics sind ausschließlich in `#[cfg(test)]`-Unittests erlaubt).
- **Eigenständige Windows-Binary**: Statische Verlinkung der MSVC C-Runtime (`+crt-static`), sodass kein Visual C++ Redistributable benötigt wird.
- **Herstellerneutral & erweiterbar**: Modulare Trait-Architektur (`BacklightBackend` / `BacklightDevice` in `src/device.rs`).
- **Standardmäßig ohne aufgezwungene RGB-Effekte**: Beim Einschalten (`Native`) wird die Helligkeit bzw. die Hardware-Stromverwaltung wieder aktiviert, sodass deine eigene Beleuchtungs-Konfiguration erhalten bleibt. Bei Razer-Extended-Geräten werden Hardware-Regionen (`0x0F:0x80`) dynamisch abgefragt und aktive Effekte (`0x0F:0x82`) wiederhergestellt, falls ein Gerät im Modus `OFF` (`0x00`) stand (optional lässt sich mit `--static R,G,B` auch eine feste Farbe setzen).

## Installation

### Vorgefertigte Binaries (Windows)

Lade das aktuelle Archiv (`x86_64-pc-windows-msvc` oder `aarch64-pc-windows-msvc`) samt `.sha256`-Prüfsumme von den [GitHub Releases](https://github.com/drlalx/hid-backlight-guard/releases) herunter und entpacke `hid-backlight-guard.exe`.

### Aus dem Quellcode bauen

Benötigt Rust `1.88+` (Edition 2024) unter Windows:

```powershell
git clone https://github.com/drlalx/hid-backlight-guard.git
cd hid-backlight-guard
cargo build --release
```

## Unterstützte Hersteller (OpenRGB-Protokoll-Port)

- **Razer (`src/backends/razer.rs`, `src/backends/razer_devices.rs`)**:
  - 187 portierte Geräte-/Interface-Profile aus OpenRGB (`Standard`, `Linear`, `Extended`, `ExtendedArgb`, `Custom`) für Tastaturen, Mäuse, Keypads, Headsets, Mousepads und Zubehör sowie ein Fallback für weitere Razer-HID-Steuerungsinterfaces.
  - Fragt Hardware-LED-Regionen (`0x0F:0x80`) bei Extended-Geräten dynamisch ab (z. B. Scrollrad-, Logo- und Daumenfeld-Zonen bei Mehrzonen-Mäusen).
- **Logitech (`src/backends/logitech.rs`)**:
  - Unified **HID++ 2.0** (`0x8071` RGB Effects Power Management, `0x8070` Color LED Effects, `0x0620` Headset Hostmode, `0x8081` Per-Key) inklusive direkter USB-/Bluetooth-Geräte und Funkempfänger-Slots (`1..=6`), plus Legacy-Fallback für ältere G-Series-/Lightsync-Geräte (z. B. G213, G813/G815, G203).

## Verwendung

```powershell
# Erkannte unterstützte HID-Geräte auflisten
hid-backlight-guard.exe --list

# Hardware-Regionen, Helligkeit und aktuellen Effekt abfragen
hid-backlight-guard.exe --probe

# Beleuchtung manuell ausschalten / wiederherstellen
hid-backlight-guard.exe --off
hid-backlight-guard.exe --on
hid-backlight-guard.exe --on --static 255,255,255

# Hintergrund-Überwachung starten (Standard ohne Parameter)
hid-backlight-guard.exe --background
hid-backlight-guard.exe --background --silent

# Windows-Autostart in HKCU\Software\Microsoft\Windows\CurrentVersion\Run verwalten
hid-backlight-guard.exe --install-autostart
hid-backlight-guard.exe --uninstall-autostart
```

## Entwicklung & Tests

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release
```

### Neue Hersteller hinzufügen

1. Neues Modul unter `src/backends/<hersteller>.rs` anlegen.
2. `BacklightBackend` (Geräteerkennung über `hidapi::HidApi`) und `BacklightDevice` (`set_backlight_off` / `set_backlight_on`) implementieren.
3. Das Backend in `src/backends/mod.rs` innerhalb `default_backends()` registrieren.

## Lizenz

Lizenziert unter der [GNU General Public License v2.0 or later](LICENSE) (`GPL-2.0-or-later`).
Protokolldefinitionen und Geräte-Deskriptoren basieren auf [OpenRGB](https://gitlab.com/CalcProgrammer1/OpenRGB).
