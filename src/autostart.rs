//! Pure-Rust Windows HKCU Run registry autostart helper (no external tools).

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_SET_VALUE, REG_SZ,
};

use crate::device::GuardError;

const RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "HidBacklightGuard";

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Registers the current executable with `--background --silent` under HKCU Run.
pub fn install_autostart() -> Result<String, GuardError> {
    let exe_path = std::env::current_exe()?;
    let command = format!("\"{}\" --background --silent", exe_path.display());

    let subkey_w = to_wide_null(RUN_SUBKEY);
    let name_w = to_wide_null(VALUE_NAME);
    let data_w = to_wide_null(&command);

    let mut hkey: HKEY = std::ptr::null_mut();
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

    let byte_len = u32::try_from(data_w.len().saturating_mul(2))
        .map_err(|_| GuardError::InvalidArgument("autostart path exceeds Windows registry limit"))?;
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
    unsafe { RegCloseKey(hkey) };

    if write_status != ERROR_SUCCESS {
        return Err(GuardError::Win32 {
            op: "RegSetValueExW",
            code: write_status,
        });
    }

    Ok(command)
}

/// Removes the `HidBacklightGuard` entry from HKCU Run.
pub fn uninstall_autostart() -> Result<(), GuardError> {
    let subkey_w = to_wide_null(RUN_SUBKEY);
    let name_w = to_wide_null(VALUE_NAME);

    let mut hkey: HKEY = std::ptr::null_mut();
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

    let del_status = unsafe { RegDeleteValueW(hkey, name_w.as_ptr()) };
    unsafe { RegCloseKey(hkey) };

    if del_status != ERROR_SUCCESS {
        return Err(GuardError::Win32 {
            op: "RegDeleteValueW",
            code: del_status,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_string_ends_with_nul() {
        let wide = to_wide_null("abc");
        assert_eq!(wide, vec![b'a' as u16, b'b' as u16, b'c' as u16, 0]);
    }
}
