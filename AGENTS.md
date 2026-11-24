# AGENTS Memory - hid-backlight-guard

## Globale Arbeitsweise

- Antworte immer auf Deutsch, außer Code (dieser soll auf Englisch sein)
- Halte Änderungen minimal und fokussiert (Atomic Commits)
- Bevorzuge funktionale Programmierung gegenüber OOP
- Test-driven Development: Neue Funktionalität zuerst durch Tests beschreiben und dann umsetzen
- Halte die Dokumentation aktuell
- Optimiere Token und Kontext aktiv mit RTK, Headroom und LLMLingua

## Projekt-Besonderheiten

- **Plattform:** Plattformübergreifendes CLI-/Hintergrund-Tool für **Windows** (`windows-sys` Win32 Registry & `WM_POWERBROADCAST`), **Linux** (`hidapi` `linux-native-basic-udev`, D-Bus via `zbus`, `systemd --user` / XDG) und **macOS** (Apple `IOKit` / `CoreFoundation` FFI & `LaunchAgent`).
- **Unsafe-Policy:** `unsafe_code = "deny"` in `Cargo.toml`; nur für zwingende OS-FFI-Aufrufe in `src/autostart.rs` (Windows) und `src/power_monitor.rs` (Windows & macOS) mit lokalem `#[allow(unsafe_code)]` und `// SAFETY:`-Kommentar erlaubt (`clippy::undocumented_unsafe_blocks = "deny"`).
- **Lizenz:** `GPL-2.0-or-later` (wegen portierter OpenRGB-Protokoll-/Gerätetabellen); alle Drittanbieter-Dependencies bleiben permissiv (`deny.toml` + `tests/license_compliance.rs`).

## Build-, Test- & Compliance-Befehle

- **Build:** `cargo build`
- **Formatierung prüfen:** `cargo fmt --all -- --check`
- **Linting (No-Panic & Safety):** `cargo clippy --all-targets -- -D warnings`
- **Tests ausführen:** `cargo test --all-targets`
- **Lizenzprüfung:** `cargo test --test license_compliance` (oder `cargo deny check licenses` via `deny.toml`)
- **CVE-/Sicherheitsprüfung:** `cargo test --test test_security_advisories` (oder `cargo deny check advisories` via `deny.toml`)

## Gold-Standard-Referenz

Siehe [rust-project-checklist.md](file:///home/adrichel/develop/tips-and-tricks/rust-project-checklist.md).
