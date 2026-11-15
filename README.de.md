# hid-backlight-guard

> [English version (README.md)](README.md)

Ein schlankes, komplett in **pure Rust** geschriebenes Windows-Dienstprogramm, das die Hintergrundbeleuchtung unterstützter HID-Peripheriegeräte (Tastaturen, Mäuse, Headsets, Mousepads usw.) automatisch ausschaltet, sobald der Bildschirm abgeschaltet wird oder der PC in den Energiesparmodus wechselt – und beim Aufwecken wiederherstellt.

## Merkmale

- **Pure Rust, Zero-Panic in Produktivcode**: Keine `.unwrap()`, `.expect()`, `panic!()` oder ungeprüften Index-Zugriffe im Produktivcode (durch `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::todo, clippy::unimplemented, clippy::unreachable, clippy::indexing_slicing)]` erzwungen; Panics sind ausschließlich in `#[cfg(test)]`-Unittests erlaubt).
- **Herstellerneutral & erweiterbar**: Modulare Trait-Architektur (`BacklightBackend` / `BacklightDevice` in `src/device.rs`).
- **Standardmäßig ohne aufgezwungene RGB-Effekte**: Beim Einschalten (`Native`) wird die Helligkeit bzw. die Hardware-Stromverwaltung wieder aktiviert, sodass deine eigene Beleuchtungs-Konfiguration erhalten bleibt. Bei Razer-Extended-Geräten werden Hardware-Regionen (`0x0F:0x80`) dynamisch abgefragt und aktive Effekte (`0x0F:0x82`) wiederhergestellt, falls ein Gerät im Modus `OFF` (`0x00`) stand (optional lässt sich mit `--static R,G,B` auch eine feste Farbe setzen).

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
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

### Neue Hersteller hinzufügen

1. Neues Modul unter `src/backends/<hersteller>.rs` anlegen.
2. `BacklightBackend` (Geräteerkennung über `hidapi::HidApi`) und `BacklightDevice` (`set_backlight_off` / `set_backlight_on`) implementieren.
3. Das Backend in `src/backends/mod.rs` innerhalb `default_backends()` registrieren.
