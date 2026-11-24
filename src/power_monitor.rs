//! Windows display and power state listener using a message-only window (`WM_POWERBROADCAST`).

use std::sync::atomic::{AtomicBool, Ordering};

use hidapi::HidApi;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Console::GetConsoleWindow;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Power::{
    HPOWERNOTIFY, POWERBROADCAST_SETTING, RegisterPowerSettingNotification,
    UnregisterPowerSettingNotification,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DEVICE_NOTIFY_WINDOW_HANDLE, DefWindowProcW, DispatchMessageW,
    GetMessageW, HWND_MESSAGE, MSG, PBT_APMRESUMEAUTOMATIC, PBT_APMSUSPEND, PBT_POWERSETTINGCHANGE,
    PostQuitMessage, RegisterClassW, SW_HIDE, ShowWindow, TranslateMessage, WM_DESTROY,
    WM_POWERBROADCAST, WNDCLASSW,
};
use windows_sys::core::GUID;

use crate::backends::discover_all_devices;
use crate::device::{GuardError, RestoreMode};

// {6FE69556-704A-47A0-8F24-C28D936FDA47}
const GUID_CONSOLE_DISPLAY_STATE: GUID = GUID::from_u128(0x6fe69556_704a_47a0_8f24_c28d936fda47);
// {2B84FAC1-4C88-4F57-887E-6364B8E28610}
const GUID_SESSION_DISPLAY_STATUS: GUID = GUID::from_u128(0x2b84fac1_4c88_4f57_887e_6364b8e28610);

static IS_DISPLAY_OFF: AtomicBool = AtomicBool::new(false);
static RESTORE_RED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static RESTORE_GREEN: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static RESTORE_BLUE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static RESTORE_STATIC: AtomicBool = AtomicBool::new(false);

fn current_restore_mode() -> RestoreMode {
    if RESTORE_STATIC.load(Ordering::SeqCst) {
        RestoreMode::Static {
            r: RESTORE_RED.load(Ordering::SeqCst),
            g: RESTORE_GREEN.load(Ordering::SeqCst),
            b: RESTORE_BLUE.load(Ordering::SeqCst),
        }
    } else {
        RestoreMode::Native
    }
}

/// Applies backlight OFF or ON across every currently connected device.
pub fn apply_to_all(turn_on: bool, restore: RestoreMode) -> Result<usize, GuardError> {
    let api = HidApi::new()?;
    let mut devices = discover_all_devices(&api)?;
    let mut succeeded = 0usize;

    for device in &mut devices {
        let info = device.info().clone();
        let res = if turn_on {
            device.set_backlight_on(restore)
        } else {
            device.set_backlight_off()
        };

        match res {
            Ok(()) => {
                succeeded = succeeded.saturating_add(1);
            }
            Err(err) => {
                eprintln!(
                    "[{} {:04X}:{:04X}] Fehler beim Umschalten ({}): {err}",
                    info.name,
                    info.vid,
                    info.pid,
                    if turn_on { "EIN" } else { "AUS" }
                );
            }
        }
    }

    Ok(succeeded)
}

fn handle_display_state(state: u32) {
    // 0 = Off, 1 = On, 2 = Dimmed
    let restore = current_restore_mode();
    if state == 0 {
        if !IS_DISPLAY_OFF.swap(true, Ordering::SeqCst) {
            if let Err(err) = apply_to_all(false, restore) {
                eprintln!("Fehler beim Ausschalten der Beleuchtung: {err}");
            }
        }
    } else if state == 1 && IS_DISPLAY_OFF.swap(false, Ordering::SeqCst) {
        if let Err(err) = apply_to_all(true, restore) {
            eprintln!("Fehler beim Einschalten der Beleuchtung: {err}");
        }
    }
}

fn guid_eq(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

#[allow(unsafe_code)]
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_POWERBROADCAST => {
            let event = wparam as u32;
            if event == PBT_POWERSETTINGCHANGE {
                let setting_ptr = lparam as *const POWERBROADCAST_SETTING;
                if !setting_ptr.is_null() {
                    // SAFETY: Windows passes a valid `POWERBROADCAST_SETTING` pointer in `lparam`
                    // for `PBT_POWERSETTINGCHANGE`, and we verified it is non-null.
                    let setting = unsafe { &*setting_ptr };
                    if (guid_eq(&setting.PowerSetting, &GUID_CONSOLE_DISPLAY_STATE)
                        || guid_eq(&setting.PowerSetting, &GUID_SESSION_DISPLAY_STATUS))
                        && setting.DataLength >= 4
                    {
                        // SAFETY: `setting.DataLength >= 4` guarantees at least 4 readable bytes
                        // starting at `setting.Data`.
                        let bytes = unsafe { std::slice::from_raw_parts(setting.Data.as_ptr(), 4) };
                        if let (Some(&b0), Some(&b1), Some(&b2), Some(&b3)) =
                            (bytes.first(), bytes.get(1), bytes.get(2), bytes.get(3))
                        {
                            let state = u32::from_le_bytes([b0, b1, b2, b3]);
                            handle_display_state(state);
                        }
                    }
                }
            } else if event == PBT_APMSUSPEND {
                handle_display_state(0);
            } else if event == PBT_APMRESUMEAUTOMATIC {
                handle_display_state(1);
            }
            1
        }
        WM_DESTROY => {
            // SAFETY: Standard Win32 shutdown signal inside `WM_DESTROY`.
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => {
            // SAFETY: Forwards unhandled messages to the default Win32 window procedure.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

/// Runs the blocking Windows message loop until terminated.
#[allow(unsafe_code)]
pub fn run_background_monitor(hide_console: bool, restore: RestoreMode) -> Result<(), GuardError> {
    match restore {
        RestoreMode::Native => RESTORE_STATIC.store(false, Ordering::SeqCst),
        RestoreMode::Static { r, g, b } => {
            RESTORE_RED.store(r, Ordering::SeqCst);
            RESTORE_GREEN.store(g, Ordering::SeqCst);
            RESTORE_BLUE.store(b, Ordering::SeqCst);
            RESTORE_STATIC.store(true, Ordering::SeqCst);
        }
    }

    if hide_console {
        // SAFETY: `GetConsoleWindow` takes no pointers and returns null if no console is attached.
        let console = unsafe { GetConsoleWindow() };
        if !console.is_null() {
            // SAFETY: `console` was checked non-null above.
            unsafe { ShowWindow(console, SW_HIDE) };
        }
    }

    // SAFETY: Passing null returns the module handle of the current executable.
    let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class_name: Vec<u16> = "HidBacklightGuardMsgClass\0".encode_utf16().collect();

    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class_name.as_ptr(),
    };

    // SAFETY: `wc` is properly initialized and `class_name` is NUL-terminated UTF-16.
    unsafe { RegisterClassW(&wc) };

    // SAFETY: `class_name` is NUL-terminated UTF-16 and `hinstance` is the current module handle.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        )
    };

    if hwnd.is_null() {
        return Err(GuardError::Win32 {
            op: "CreateWindowExW",
            code: 0,
        });
    }

    // SAFETY: `hwnd` is a valid non-null window handle and the GUID is a static constant.
    let notify1: HPOWERNOTIFY = unsafe {
        RegisterPowerSettingNotification(
            hwnd,
            &GUID_CONSOLE_DISPLAY_STATE,
            DEVICE_NOTIFY_WINDOW_HANDLE,
        )
    };
    // SAFETY: `hwnd` is a valid non-null window handle and the GUID is a static constant.
    let notify2: HPOWERNOTIFY = unsafe {
        RegisterPowerSettingNotification(
            hwnd,
            &GUID_SESSION_DISPLAY_STATUS,
            DEVICE_NOTIFY_WINDOW_HANDLE,
        )
    };

    // SAFETY: `MSG` is a plain Win32 C struct where all-zero initialization is valid.
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: `msg` is a valid mutable pointer to `MSG` for the current thread's queue.
        let ret = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if ret <= 0 {
            break;
        }
        // SAFETY: `msg` was populated by a successful `GetMessageW` call.
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    if notify1 != 0 {
        // SAFETY: `notify1` is a valid non-zero handle returned by `RegisterPowerSettingNotification`.
        unsafe { UnregisterPowerSettingNotification(notify1) };
    }
    if notify2 != 0 {
        // SAFETY: `notify2` is a valid non-zero handle returned by `RegisterPowerSettingNotification`.
        unsafe { UnregisterPowerSettingNotification(notify2) };
    }

    Ok(())
}
