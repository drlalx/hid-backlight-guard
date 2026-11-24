//! Command-line entry point for `hid-backlight-guard`.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

use std::process::ExitCode;

use hid_backlight_guard::autostart::{install_autostart, uninstall_autostart};
use hid_backlight_guard::backends::discover_all_devices;
use hid_backlight_guard::device::{GuardError, RestoreMode};
use hid_backlight_guard::power_monitor::{apply_to_all, run_background_monitor};
use hidapi::HidApi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Background,
    List,
    Probe,
    Off,
    On,
    InstallAutostart,
    UninstallAutostart,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CliConfig {
    action: Action,
    silent: bool,
    restore: RestoreMode,
}

fn parse_static_rgb(spec: &str) -> Result<RestoreMode, GuardError> {
    let mut parts = spec.split(',');
    let r_str = parts
        .next()
        .ok_or(GuardError::InvalidArgument("erwartetes Format: R,G,B"))?;
    let g_str = parts
        .next()
        .ok_or(GuardError::InvalidArgument("erwartetes Format: R,G,B"))?;
    let b_str = parts
        .next()
        .ok_or(GuardError::InvalidArgument("erwartetes Format: R,G,B"))?;
    if parts.next().is_some() {
        return Err(GuardError::InvalidArgument(
            "zu viele Farbkomponenten; erwartet R,G,B",
        ));
    }

    let r = r_str
        .trim()
        .parse::<u8>()
        .map_err(|_| GuardError::InvalidArgument("Rotwert muss zwischen 0 und 255 liegen"))?;
    let g = g_str
        .trim()
        .parse::<u8>()
        .map_err(|_| GuardError::InvalidArgument("Grünwert muss zwischen 0 und 255 liegen"))?;
    let b = b_str
        .trim()
        .parse::<u8>()
        .map_err(|_| GuardError::InvalidArgument("Blauwert muss zwischen 0 und 255 liegen"))?;

    Ok(RestoreMode::Static { r, g, b })
}

fn parse_args<I, S>(args: I) -> Result<CliConfig, GuardError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut action = Action::Background;
    let mut action_explicit = false;
    let mut silent = false;
    let mut restore = RestoreMode::Native;

    let mut iter = args.into_iter().skip(1);
    while let Some(arg) = iter.next() {
        let s = arg.as_ref();
        match s {
            "--list" | "-l" => {
                action = Action::List;
                action_explicit = true;
            }
            "--probe" => {
                action = Action::Probe;
                action_explicit = true;
            }
            "--off" => {
                action = Action::Off;
                action_explicit = true;
            }
            "--on" => {
                action = Action::On;
                action_explicit = true;
            }
            "--background" | "-b" => {
                action = Action::Background;
                action_explicit = true;
            }
            "--install-autostart" => {
                action = Action::InstallAutostart;
                action_explicit = true;
            }
            "--uninstall-autostart" => {
                action = Action::UninstallAutostart;
                action_explicit = true;
            }
            "--silent" => {
                silent = true;
            }
            "--static" => {
                let rgb = iter
                    .next()
                    .ok_or(GuardError::InvalidArgument("nach --static fehlt R,G,B"))?;
                restore = parse_static_rgb(rgb.as_ref())?;
            }
            "--help" | "-h" => {
                action = Action::Help;
                action_explicit = true;
            }
            _ => {
                return Err(GuardError::InvalidArgument(
                    "unbekanntes Argument (nutze --help für Optionen)",
                ));
            }
        }
    }

    if !action_explicit && silent {
        action = Action::Background;
    }

    Ok(CliConfig {
        action,
        silent,
        restore,
    })
}

fn print_help() {
    println!(
        "hid-backlight-guard v{}\n\
         \n\
         Nutzung: hid-backlight-guard [OPTIONEN]\n\
         \n\
         Optionen:\n\
           -b, --background           Hintergrund-Überwachung (Standard)\n\
               --silent               Konsolenfenster im Hintergrund ausblenden\n\
           -l, --list                 Erkannte kompatible HID-Geräte anzeigen\n\
               --probe                Hardware-Regionen & Helligkeit abfragen\n\
               --off                  Beleuchtung aller erkannten Geräte ausschalten\n\
               --on                   Beleuchtung aller erkannten Geräte wiederherstellen\n\
               --static R,G,B         Beim Einschalten statische Farbe setzen (z.B. 255,255,255)\n\
               --install-autostart    Autostart in Windows-Registry eintragen\n\
               --uninstall-autostart  Autostart-Eintrag entfernen\n\
           -h, --help                 Diese Hilfe anzeigen",
        env!("CARGO_PKG_VERSION")
    );
}

fn run() -> Result<(), GuardError> {
    let cfg = parse_args(std::env::args())?;

    match cfg.action {
        Action::Help => {
            print_help();
            Ok(())
        }
        Action::List => {
            let api = HidApi::new()?;
            let devices = discover_all_devices(&api)?;
            if devices.is_empty() {
                println!("Keine kompatiblen HID-Geräte gefunden.");
            } else {
                println!("Erkannte Geräte ({}):", devices.len());
                for dev in devices {
                    let info = dev.info();
                    println!(
                        "  - [{} / {}] {} ({:04X}:{:04X}) | {}",
                        info.vendor, info.kind, info.name, info.vid, info.pid, info.protocol
                    );
                }
            }
            Ok(())
        }
        Action::Probe => {
            let api = HidApi::new()?;
            let mut devices = discover_all_devices(&api)?;
            for dev in &mut devices {
                let info = dev.info().clone();
                println!(
                    "[{} / {}] {} ({:04X}:{:04X}) | {}",
                    info.vendor, info.kind, info.name, info.vid, info.pid, info.protocol
                );
                match dev.probe_details() {
                    Ok(lines) => {
                        for line in lines {
                            println!("    {line}");
                        }
                    }
                    Err(err) => println!("    Probe-Fehler: {err}"),
                }
            }
            Ok(())
        }
        Action::Off => {
            let count = apply_to_all(false, cfg.restore)?;
            println!("Beleuchtung ausgeschaltet ({count} Gerät(e)).");
            Ok(())
        }
        Action::On => {
            let count = apply_to_all(true, cfg.restore)?;
            println!("Beleuchtung eingeschaltet ({count} Gerät(e)).");
            Ok(())
        }
        Action::InstallAutostart => {
            let cmd = install_autostart()?;
            println!("Autostart installiert: {cmd}");
            Ok(())
        }
        Action::UninstallAutostart => {
            uninstall_autostart()?;
            println!("Autostart entfernt.");
            Ok(())
        }
        Action::Background => {
            if !cfg.silent {
                println!("hid-backlight-guard aktiv: Überwache Bildschirm- und Energiestatus...");
            }
            run_background_monitor(cfg.silent, cfg.restore)
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("Fehler: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_static_color_argument() {
        let cfg = parse_args(["hid-backlight-guard", "--on", "--static", "10, 200, 30"]).unwrap();
        assert_eq!(cfg.action, Action::On);
        assert_eq!(
            cfg.restore,
            RestoreMode::Static {
                r: 10,
                g: 200,
                b: 30
            }
        );
    }

    #[test]
    fn defaults_to_background_native() {
        let cfg = parse_args(["hid-backlight-guard"]).unwrap();
        assert_eq!(cfg.action, Action::Background);
        assert_eq!(cfg.restore, RestoreMode::Native);
        assert!(!cfg.silent);
    }

    #[test]
    fn rejects_out_of_range_rgb() {
        assert!(parse_static_rgb("256,0,0").is_err());
        assert!(parse_static_rgb("10,20").is_err());
    }
}
