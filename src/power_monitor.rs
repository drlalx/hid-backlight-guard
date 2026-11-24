//! Cross-platform display and power state listener:
//! - Windows: message-only window (`WM_POWERBROADCAST`)
//! - Linux: D-Bus (`systemd-logind` `PrepareForSleep` + `ScreenSaver` `ActiveChanged`)
//! - macOS: Apple `IOKit` (`IORegisterForSystemPower` + `IODisplayWrangler` notifications)

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use hidapi::HidApi;

use crate::backends::discover_all_devices;
use crate::device::{GuardError, RestoreMode};

/// Normalized display or system power transition across all supported operating systems.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    /// Display turned off or screen saver / DPMS blanking became active.
    DisplayOff,
    /// Display turned back on or screen saver / DPMS blanking ended.
    DisplayOn,
    /// System is entering suspend / sleep.
    Suspend,
    /// System resumed from suspend / sleep.
    Resume,
}

/// Pure, testable state machine preventing duplicate backlight toggles.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DisplayStateTracker {
    is_off: bool,
}

impl DisplayStateTracker {
    /// Applies a power event and returns `Some(false)` to turn backlights OFF,
    /// `Some(true)` to turn them ON, or `None` when no state change is needed.
    pub fn apply(&mut self, event: PowerEvent) -> Option<bool> {
        match event {
            PowerEvent::DisplayOff | PowerEvent::Suspend => {
                if !self.is_off {
                    self.is_off = true;
                    Some(false)
                } else {
                    None
                }
            }
            PowerEvent::DisplayOn | PowerEvent::Resume => {
                if self.is_off {
                    self.is_off = false;
                    Some(true)
                } else {
                    None
                }
            }
        }
    }
}

static IS_DISPLAY_OFF: AtomicBool = AtomicBool::new(false);
static RESTORE_RED: AtomicU8 = AtomicU8::new(0);
static RESTORE_GREEN: AtomicU8 = AtomicU8::new(0);
static RESTORE_BLUE: AtomicU8 = AtomicU8::new(0);
static RESTORE_STATIC: AtomicBool = AtomicBool::new(false);

fn store_restore_mode(restore: RestoreMode) {
    match restore {
        RestoreMode::Native => RESTORE_STATIC.store(false, Ordering::SeqCst),
        RestoreMode::Static { r, g, b } => {
            RESTORE_RED.store(r, Ordering::SeqCst);
            RESTORE_GREEN.store(g, Ordering::SeqCst);
            RESTORE_BLUE.store(b, Ordering::SeqCst);
            RESTORE_STATIC.store(true, Ordering::SeqCst);
        }
    }
}

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
                    "[{} {:04X}:{:04X}] Error toggling backlight ({}): {err}",
                    info.name,
                    info.vid,
                    info.pid,
                    if turn_on { "ON" } else { "OFF" }
                );
            }
        }
    }

    Ok(succeeded)
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
fn handle_power_event(event: PowerEvent) {
    let restore = current_restore_mode();
    match event {
        PowerEvent::DisplayOff | PowerEvent::Suspend => {
            if !IS_DISPLAY_OFF.swap(true, Ordering::SeqCst)
                && let Err(err) = apply_to_all(false, restore)
            {
                eprintln!("Error turning backlights off: {err}");
            }
        }
        PowerEvent::DisplayOn | PowerEvent::Resume => {
            if IS_DISPLAY_OFF.swap(false, Ordering::SeqCst)
                && let Err(err) = apply_to_all(true, restore)
            {
                eprintln!("Error turning backlights on: {err}");
            }
        }
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Power::{
        HPOWERNOTIFY, POWERBROADCAST_SETTING, RegisterPowerSettingNotification,
        UnregisterPowerSettingNotification,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CW_USEDEFAULT, CreateWindowExW, DEVICE_NOTIFY_WINDOW_HANDLE, DefWindowProcW,
        DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG, PBT_APMRESUMEAUTOMATIC, PBT_APMSUSPEND,
        PBT_POWERSETTINGCHANGE, PostQuitMessage, RegisterClassW, SW_HIDE, ShowWindow,
        TranslateMessage, WM_DESTROY, WM_POWERBROADCAST, WNDCLASSW,
    };
    use windows_sys::core::GUID;

    use super::{PowerEvent, handle_power_event};
    use crate::device::GuardError;

    // {6FE69556-704A-47A0-8F24-C28D936FDA47}
    const GUID_CONSOLE_DISPLAY_STATE: GUID =
        GUID::from_u128(0x6fe69556_704a_47a0_8f24_c28d936fda47);
    // {2B84FAC1-4C88-4F57-887E-6364B8E28610}
    const GUID_SESSION_DISPLAY_STATUS: GUID =
        GUID::from_u128(0x2b84fac1_4c88_4f57_887e_6364b8e28610);

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
                            let bytes =
                                unsafe { std::slice::from_raw_parts(setting.Data.as_ptr(), 4) };
                            if let (Some(&b0), Some(&b1), Some(&b2), Some(&b3)) =
                                (bytes.first(), bytes.get(1), bytes.get(2), bytes.get(3))
                            {
                                // 0 = Off, 1 = On, 2 = Dimmed
                                match u32::from_le_bytes([b0, b1, b2, b3]) {
                                    0 => handle_power_event(PowerEvent::DisplayOff),
                                    1 => handle_power_event(PowerEvent::DisplayOn),
                                    _ => {}
                                }
                            }
                        }
                    }
                } else if event == PBT_APMSUSPEND {
                    handle_power_event(PowerEvent::Suspend);
                } else if event == PBT_APMRESUMEAUTOMATIC {
                    handle_power_event(PowerEvent::Resume);
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

    #[allow(unsafe_code)]
    pub fn run_background_monitor(hide_console: bool) -> Result<(), GuardError> {
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
}

#[cfg(target_os = "linux")]
mod platform {
    use std::thread;

    use zbus::MatchRule;
    use zbus::blocking::{Connection, MessageIterator};
    use zbus::message::Type as MessageType;

    use super::{PowerEvent, handle_power_event};
    use crate::device::GuardError;

    const SCREENSAVER_INTERFACES: &[&str] = &[
        "org.freedesktop.ScreenSaver",
        "org.gnome.ScreenSaver",
        "org.cinnamon.ScreenSaver",
        "org.mate.ScreenSaver",
        "org.xfce.ScreenSaver",
    ];

    fn listen_system_bus(conn: Connection) {
        let Ok(rule) = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .interface("org.freedesktop.login1.Manager")
            .and_then(|b| b.member("PrepareForSleep"))
            .map(zbus::match_rule::Builder::build)
        else {
            return;
        };

        let Ok(stream) = MessageIterator::for_match_rule(rule, &conn, None) else {
            return;
        };

        for msg_res in stream {
            let Ok(msg) = msg_res else {
                continue;
            };
            if let Ok(going_to_sleep) = msg.body().deserialize::<bool>() {
                if going_to_sleep {
                    handle_power_event(PowerEvent::Suspend);
                } else {
                    handle_power_event(PowerEvent::Resume);
                }
            }
        }
    }

    fn listen_session_bus(conn: Connection) {
        let Ok(rule) = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .member("ActiveChanged")
            .map(zbus::match_rule::Builder::build)
        else {
            return;
        };

        let Ok(stream) = MessageIterator::for_match_rule(rule, &conn, None) else {
            return;
        };

        for msg_res in stream {
            let Ok(msg) = msg_res else {
                continue;
            };
            let header = msg.header();
            let iface_matches = header
                .interface()
                .is_some_and(|iface| SCREENSAVER_INTERFACES.contains(&iface.as_str()));
            if !iface_matches {
                continue;
            }
            if let Ok(active) = msg.body().deserialize::<bool>() {
                if active {
                    handle_power_event(PowerEvent::DisplayOff);
                } else {
                    handle_power_event(PowerEvent::DisplayOn);
                }
            }
        }
    }

    pub fn run_background_monitor(_hide_console: bool) -> Result<(), GuardError> {
        let system_conn = Connection::system().ok();
        let session_conn = Connection::session().ok();

        if system_conn.is_none() && session_conn.is_none() {
            return Err(GuardError::Platform {
                op: "dbus",
                detail: "could not connect to either D-Bus system bus or session bus".to_owned(),
            });
        }

        let mut handles = Vec::new();
        if let Some(conn) = system_conn {
            handles.push(thread::spawn(move || listen_system_bus(conn)));
        }
        if let Some(conn) = session_conn {
            handles.push(thread::spawn(move || listen_session_bus(conn)));
        }

        for handle in handles {
            let _ = handle.join();
        }

        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::{c_char, c_void};
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::{PowerEvent, handle_power_event};
    use crate::device::GuardError;

    type IoConnect = u32;
    type IoObject = u32;
    type IoService = u32;
    type KernReturn = i32;
    type MachPort = u32;
    type CfRunLoopRef = *mut c_void;
    type CfRunLoopSourceRef = *mut c_void;
    type CfStringRef = *const c_void;
    type CfMutableDictionaryRef = *mut c_void;
    type IoNotificationPortRef = *mut c_void;

    type IoServiceInterestCallback = unsafe extern "C" fn(
        refcon: *mut c_void,
        service: IoService,
        message_type: u32,
        message_argument: *mut c_void,
    );

    const K_IO_MESSAGE_CAN_SYSTEM_SLEEP: u32 = 0xE000_0270;
    const K_IO_MESSAGE_SYSTEM_WILL_SLEEP: u32 = 0xE000_0280;
    const K_IO_MESSAGE_DEVICE_WILL_POWER_OFF: u32 = 0xE000_0250;
    const K_IO_MESSAGE_SYSTEM_HAS_POWERED_ON: u32 = 0xE000_0300;
    const K_IO_GENERAL_INTEREST: &[u8] = b"IOGeneralInterest\0";
    const IO_DISPLAY_WRANGLER: &[u8] = b"IODisplayWrangler\0";

    static ROOT_PORT: AtomicU32 = AtomicU32::new(0);

    #[link(name = "IOKit", kind = "framework")]
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn IORegisterForSystemPower(
            refcon: *mut c_void,
            the_port_ref: *mut IoNotificationPortRef,
            callback: IoServiceInterestCallback,
            notifier: *mut IoObject,
        ) -> IoConnect;
        fn IOAllowPowerChange(kernel_port: IoConnect, notification_id: isize) -> KernReturn;
        fn IONotificationPortCreate(main_port: MachPort) -> IoNotificationPortRef;
        fn IONotificationPortGetRunLoopSource(notify: IoNotificationPortRef) -> CfRunLoopSourceRef;
        fn IOServiceMatching(name: *const c_char) -> CfMutableDictionaryRef;
        fn IOServiceGetMatchingService(
            main_port: MachPort,
            matching: CfMutableDictionaryRef,
        ) -> IoService;
        fn IOServiceAddInterestNotification(
            notify_port: IoNotificationPortRef,
            service: IoService,
            interest_type: *const c_char,
            callback: IoServiceInterestCallback,
            refcon: *mut c_void,
            notification: *mut IoObject,
        ) -> KernReturn;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    #[allow(unsafe_code)]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: CfStringRef;
        fn CFRunLoopGetCurrent() -> CfRunLoopRef;
        fn CFRunLoopAddSource(rl: CfRunLoopRef, source: CfRunLoopSourceRef, mode: CfStringRef);
        fn CFRunLoopRun();
    }

    #[allow(unsafe_code)]
    unsafe extern "C" fn system_power_callback(
        _refcon: *mut c_void,
        _service: IoService,
        message_type: u32,
        message_argument: *mut c_void,
    ) {
        let root_port = ROOT_PORT.load(Ordering::SeqCst);
        match message_type {
            K_IO_MESSAGE_CAN_SYSTEM_SLEEP => {
                if root_port != 0 {
                    // SAFETY: `root_port` is the valid `io_connect_t` from `IORegisterForSystemPower`
                    // and `message_argument` is the notification token supplied by IOKit.
                    unsafe { IOAllowPowerChange(root_port, message_argument as isize) };
                }
            }
            K_IO_MESSAGE_SYSTEM_WILL_SLEEP => {
                handle_power_event(PowerEvent::Suspend);
                if root_port != 0 {
                    // SAFETY: Acknowledges sleep so macOS does not stall for 30 seconds.
                    unsafe { IOAllowPowerChange(root_port, message_argument as isize) };
                }
            }
            K_IO_MESSAGE_SYSTEM_HAS_POWERED_ON => {
                handle_power_event(PowerEvent::Resume);
            }
            _ => {}
        }
    }

    #[allow(unsafe_code)]
    unsafe extern "C" fn display_power_callback(
        _refcon: *mut c_void,
        _service: IoService,
        message_type: u32,
        _message_argument: *mut c_void,
    ) {
        match message_type {
            K_IO_MESSAGE_DEVICE_WILL_POWER_OFF => {
                handle_power_event(PowerEvent::DisplayOff);
            }
            K_IO_MESSAGE_SYSTEM_HAS_POWERED_ON => {
                handle_power_event(PowerEvent::DisplayOn);
            }
            _ => {}
        }
    }

    #[allow(unsafe_code)]
    pub fn run_background_monitor(_hide_console: bool) -> Result<(), GuardError> {
        let mut power_notify_port: IoNotificationPortRef = std::ptr::null_mut();
        let mut power_notifier: IoObject = 0;

        // SAFETY: Output pointers are valid and `system_power_callback` matches IOKit's signature.
        let root_port = unsafe {
            IORegisterForSystemPower(
                std::ptr::null_mut(),
                &mut power_notify_port,
                system_power_callback,
                &mut power_notifier,
            )
        };
        if root_port == 0 || power_notify_port.is_null() {
            return Err(GuardError::Platform {
                op: "IORegisterForSystemPower",
                detail: "failed to register for macOS system power notifications".to_owned(),
            });
        }
        ROOT_PORT.store(root_port, Ordering::SeqCst);

        // SAFETY: Runs on the current thread's CoreFoundation run loop.
        let run_loop = unsafe { CFRunLoopGetCurrent() };
        // SAFETY: `power_notify_port` was checked non-null above.
        let power_source = unsafe { IONotificationPortGetRunLoopSource(power_notify_port) };
        // SAFETY: `run_loop`, `power_source`, and `kCFRunLoopDefaultMode` are valid CF references.
        unsafe { CFRunLoopAddSource(run_loop, power_source, kCFRunLoopDefaultMode) };

        // Also subscribe to IODisplayWrangler for display sleep/wake notifications.
        // SAFETY: `IO_DISPLAY_WRANGLER` is NUL-terminated ASCII.
        let matching = unsafe { IOServiceMatching(IO_DISPLAY_WRANGLER.as_ptr().cast::<c_char>()) };
        if !matching.is_null() {
            // SAFETY: Passing 0 uses the default IOKit main port; `matching` is consumed by the call.
            let wrangler = unsafe { IOServiceGetMatchingService(0, matching) };
            if wrangler != 0 {
                // SAFETY: 0 is the default IOKit main port.
                let display_port = unsafe { IONotificationPortCreate(0) };
                if !display_port.is_null() {
                    let mut display_notifier: IoObject = 0;
                    // SAFETY: All pointers and NUL-terminated interest type are valid.
                    let kr = unsafe {
                        IOServiceAddInterestNotification(
                            display_port,
                            wrangler,
                            K_IO_GENERAL_INTEREST.as_ptr().cast::<c_char>(),
                            display_power_callback,
                            std::ptr::null_mut(),
                            &mut display_notifier,
                        )
                    };
                    if kr == 0 {
                        // SAFETY: `display_port` is non-null and registered successfully.
                        let display_source =
                            unsafe { IONotificationPortGetRunLoopSource(display_port) };
                        // SAFETY: Adds the display notification source to the current run loop.
                        unsafe {
                            CFRunLoopAddSource(run_loop, display_source, kCFRunLoopDefaultMode)
                        };
                    }
                }
            }
        }

        // SAFETY: Enters the blocking CoreFoundation event loop for this thread.
        unsafe { CFRunLoopRun() };
        Ok(())
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod platform {
    use crate::device::GuardError;

    pub fn run_background_monitor(_hide_console: bool) -> Result<(), GuardError> {
        Err(GuardError::Platform {
            op: "run_background_monitor",
            detail: "background power monitoring is not supported on this operating system"
                .to_owned(),
        })
    }
}

/// Runs the blocking OS display/power event loop until terminated.
pub fn run_background_monitor(hide_console: bool, restore: RestoreMode) -> Result<(), GuardError> {
    store_restore_mode(restore);
    platform::run_background_monitor(hide_console)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_turns_off_once_and_restores_on_wake() {
        let mut tracker = DisplayStateTracker::default();

        assert_eq!(tracker.apply(PowerEvent::DisplayOn), None);
        assert_eq!(tracker.apply(PowerEvent::DisplayOff), Some(false));
        assert_eq!(tracker.apply(PowerEvent::Suspend), None);
        assert_eq!(tracker.apply(PowerEvent::Resume), Some(true));
        assert_eq!(tracker.apply(PowerEvent::DisplayOn), None);
    }

    #[test]
    fn tracker_handles_suspend_then_display_on() {
        let mut tracker = DisplayStateTracker::default();

        assert_eq!(tracker.apply(PowerEvent::Suspend), Some(false));
        assert_eq!(tracker.apply(PowerEvent::DisplayOff), None);
        assert_eq!(tracker.apply(PowerEvent::DisplayOn), Some(true));
    }
}
