//! Cross-platform autostart management:
//! - Windows: HKCU `Run` registry key (Win32 FFI)
//! - Linux: `systemd --user` service with XDG `.desktop` fallback
//! - macOS: `~/Library/LaunchAgents/com.hid-backlight-guard.plist`

use std::path::Path;

use crate::device::GuardError;

#[cfg(any(windows, test))]
fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Escapes XML special characters for macOS LaunchAgent `.plist` values.
#[cfg(any(target_os = "macos", test))]
fn escape_xml(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// Renders the Linux `systemd --user` unit file for `hid-backlight-guard`.
#[cfg(any(target_os = "linux", test))]
pub fn render_systemd_user_unit(exe_path: &Path) -> String {
    let escaped = exe_path.display().to_string().replace('"', "\\\"");
    format!(
        "[Unit]\n\
         Description=HID Backlight Guard (turn off peripheral backlights on display off / suspend)\n\
         After=graphical-session.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart=\"{escaped}\" --background --silent\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

/// Renders an XDG autostart `.desktop` entry for non-systemd Linux environments.
#[cfg(any(target_os = "linux", test))]
pub fn render_xdg_desktop_entry(exe_path: &Path) -> String {
    let escaped = exe_path.display().to_string().replace('"', "\\\"");
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=hid-backlight-guard\n\
         Comment=Automatically turn off HID peripheral backlights on display off or suspend\n\
         Exec=\"{escaped}\" --background --silent\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n"
    )
}

/// Renders the macOS LaunchAgent property list (`com.hid-backlight-guard.plist`).
#[cfg(any(target_os = "macos", test))]
pub fn render_launch_agent_plist(exe_path: &Path) -> String {
    let escaped = escape_xml(&exe_path.display().to_string());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
             <key>Label</key>\n\
             <string>com.hid-backlight-guard</string>\n\
             <key>ProgramArguments</key>\n\
             <array>\n\
                 <string>{escaped}</string>\n\
                 <string>--background</string>\n\
                 <string>--silent</string>\n\
             </array>\n\
             <key>RunAtLoad</key>\n\
             <true/>\n\
             <key>KeepAlive</key>\n\
             <true/>\n\
         </dict>\n\
         </plist>\n"
    )
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW,
        RegOpenKeyExW, RegSetValueExW,
    };

    use super::to_wide_null;
    use crate::device::GuardError;

    const RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const VALUE_NAME: &str = "HidBacklightGuard";

    #[allow(unsafe_code)]
    pub fn install_autostart() -> Result<String, GuardError> {
        let exe_path = std::env::current_exe()?;
        let command = format!("\"{}\" --background --silent", exe_path.display());

        let subkey_w = to_wide_null(RUN_SUBKEY);
        let name_w = to_wide_null(VALUE_NAME);
        let data_w = to_wide_null(&command);

        let mut hkey: HKEY = std::ptr::null_mut();
        // SAFETY: `subkey_w` is NUL-terminated UTF-16 and `hkey` is a valid out-pointer.
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey_w.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(GuardError::Win32 {
                op: "RegOpenKeyExW",
                code: status,
            });
        }

        let byte_len = u32::try_from(data_w.len().saturating_mul(2)).map_err(|_| {
            GuardError::InvalidArgument("autostart path exceeds Windows registry limit")
        })?;
        // SAFETY: `hkey` is a valid open registry handle; `name_w` and `data_w` are NUL-terminated
        // UTF-16 buffers whose byte length matches `byte_len`.
        let write_status = unsafe {
            RegSetValueExW(
                hkey,
                name_w.as_ptr(),
                0,
                REG_SZ,
                data_w.as_ptr().cast::<u8>(),
                byte_len,
            )
        };
        // SAFETY: `hkey` was opened by `RegOpenKeyExW` above and is closed exactly once.
        unsafe { RegCloseKey(hkey) };

        if write_status != ERROR_SUCCESS {
            return Err(GuardError::Win32 {
                op: "RegSetValueExW",
                code: write_status,
            });
        }

        Ok(command)
    }

    #[allow(unsafe_code)]
    pub fn uninstall_autostart() -> Result<(), GuardError> {
        let subkey_w = to_wide_null(RUN_SUBKEY);
        let name_w = to_wide_null(VALUE_NAME);

        let mut hkey: HKEY = std::ptr::null_mut();
        // SAFETY: `subkey_w` is NUL-terminated UTF-16 and `hkey` is a valid out-pointer.
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey_w.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(GuardError::Win32 {
                op: "RegOpenKeyExW",
                code: status,
            });
        }

        // SAFETY: `hkey` is a valid open registry handle and `name_w` is NUL-terminated UTF-16.
        let del_status = unsafe { RegDeleteValueW(hkey, name_w.as_ptr()) };
        // SAFETY: `hkey` was opened by `RegOpenKeyExW` above and is closed exactly once.
        unsafe { RegCloseKey(hkey) };

        if del_status != ERROR_SUCCESS {
            return Err(GuardError::Win32 {
                op: "RegDeleteValueW",
                code: del_status,
            });
        }

        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::path::PathBuf;
    use std::process::Command;

    use super::{render_systemd_user_unit, render_xdg_desktop_entry};
    use crate::device::GuardError;

    const SERVICE_NAME: &str = "hid-backlight-guard.service";
    const DESKTOP_NAME: &str = "hid-backlight-guard.desktop";

    fn xdg_config_home() -> Result<PathBuf, GuardError> {
        if let Ok(dir) = std::env::var("XDG_CONFIG_HOME")
            && !dir.is_empty()
        {
            return Ok(PathBuf::from(dir));
        }
        let home = std::env::var("HOME").map_err(|_| GuardError::Platform {
            op: "autostart",
            detail: "HOME environment variable is not set".to_owned(),
        })?;
        Ok(PathBuf::from(home).join(".config"))
    }

    pub fn install_autostart() -> Result<String, GuardError> {
        let exe_path = std::env::current_exe()?;
        let config_dir = xdg_config_home()?;

        let systemd_dir = config_dir.join("systemd").join("user");
        std::fs::create_dir_all(&systemd_dir)?;
        let unit_path = systemd_dir.join(SERVICE_NAME);
        std::fs::write(&unit_path, render_systemd_user_unit(&exe_path))?;

        let reload_ok = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status()
            .is_ok_and(|s| s.success());
        let enable_ok = reload_ok
            && Command::new("systemctl")
                .args(["--user", "enable", "--now", SERVICE_NAME])
                .status()
                .is_ok_and(|s| s.success());

        if enable_ok {
            return Ok(unit_path.display().to_string());
        }

        // Fallback to XDG autostart `.desktop` entry when systemd --user is unavailable.
        let autostart_dir = config_dir.join("autostart");
        std::fs::create_dir_all(&autostart_dir)?;
        let desktop_path = autostart_dir.join(DESKTOP_NAME);
        std::fs::write(&desktop_path, render_xdg_desktop_entry(&exe_path))?;
        Ok(desktop_path.display().to_string())
    }

    pub fn uninstall_autostart() -> Result<(), GuardError> {
        let config_dir = xdg_config_home()?;
        let unit_path = config_dir.join("systemd").join("user").join(SERVICE_NAME);
        let desktop_path = config_dir.join("autostart").join(DESKTOP_NAME);

        let _ = Command::new("systemctl")
            .args(["--user", "disable", "--now", SERVICE_NAME])
            .status();

        let mut removed_any = false;
        if unit_path.exists() {
            std::fs::remove_file(&unit_path)?;
            removed_any = true;
            let _ = Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .status();
        }
        if desktop_path.exists() {
            std::fs::remove_file(&desktop_path)?;
            removed_any = true;
        }

        if !removed_any {
            return Err(GuardError::Platform {
                op: "uninstall_autostart",
                detail: "no systemd user service or XDG autostart entry found".to_owned(),
            });
        }

        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::PathBuf;
    use std::process::Command;

    use super::render_launch_agent_plist;
    use crate::device::GuardError;

    const PLIST_NAME: &str = "com.hid-backlight-guard.plist";

    fn plist_path() -> Result<PathBuf, GuardError> {
        let home = std::env::var("HOME").map_err(|_| GuardError::Platform {
            op: "autostart",
            detail: "HOME environment variable is not set".to_owned(),
        })?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("LaunchAgents")
            .join(PLIST_NAME))
    }

    pub fn install_autostart() -> Result<String, GuardError> {
        let exe_path = std::env::current_exe()?;
        let path = plist_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, render_launch_agent_plist(&exe_path))?;

        let _ = Command::new("launchctl")
            .args(["load", "-w"])
            .arg(&path)
            .status();

        Ok(path.display().to_string())
    }

    pub fn uninstall_autostart() -> Result<(), GuardError> {
        let path = plist_path()?;
        let _ = Command::new("launchctl")
            .args(["unload", "-w"])
            .arg(&path)
            .status();

        if !path.exists() {
            return Err(GuardError::Platform {
                op: "uninstall_autostart",
                detail: format!("LaunchAgent not found: {}", path.display()),
            });
        }

        std::fs::remove_file(&path)?;
        Ok(())
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod platform {
    use crate::device::GuardError;

    pub fn install_autostart() -> Result<String, GuardError> {
        Err(GuardError::Platform {
            op: "install_autostart",
            detail: "autostart is not supported on this operating system".to_owned(),
        })
    }

    pub fn uninstall_autostart() -> Result<(), GuardError> {
        Err(GuardError::Platform {
            op: "uninstall_autostart",
            detail: "autostart is not supported on this operating system".to_owned(),
        })
    }
}

/// Registers the current executable with `--background --silent` in the OS autostart mechanism.
pub fn install_autostart() -> Result<String, GuardError> {
    platform::install_autostart()
}

/// Removes the `hid-backlight-guard` entry from the OS autostart mechanism.
pub fn uninstall_autostart() -> Result<(), GuardError> {
    platform::uninstall_autostart()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn wide_string_ends_with_nul() {
        let wide = to_wide_null("abc");
        assert_eq!(wide, vec![b'a' as u16, b'b' as u16, b'c' as u16, 0]);
    }

    #[test]
    fn renders_systemd_user_unit() {
        let unit = render_systemd_user_unit(&PathBuf::from("/usr/local/bin/hid-backlight-guard"));
        assert!(
            unit.contains("ExecStart=\"/usr/local/bin/hid-backlight-guard\" --background --silent")
        );
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn renders_xdg_desktop_entry() {
        let entry = render_xdg_desktop_entry(&PathBuf::from("/usr/local/bin/hid-backlight-guard"));
        assert!(entry.contains("[Desktop Entry]"));
        assert!(
            entry.contains("Exec=\"/usr/local/bin/hid-backlight-guard\" --background --silent")
        );
    }

    #[test]
    fn renders_escaped_launch_agent_plist() {
        let plist =
            render_launch_agent_plist(&PathBuf::from("/Applications/R&D/hid-backlight-guard"));
        assert!(plist.contains("<string>com.hid-backlight-guard</string>"));
        assert!(plist.contains("<string>/Applications/R&amp;D/hid-backlight-guard</string>"));
        assert!(plist.contains("<string>--background</string>"));
    }
}
