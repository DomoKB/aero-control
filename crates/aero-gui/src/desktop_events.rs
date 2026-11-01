//! Desktop power notifications and an opt-in notification-area menu. The
//! worker forwards commands; it never applies profiles or touches hardware.

use aero_core::FanMode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopEvent {
    Resume,
    PowerChanged,
    Show,
    FanMode(FanMode),
    FanHotkey,
    WifiHotkey,
    DisplayOffHotkey,
    Exit,
}

enum DesktopCommand {
    Tray(bool),
    FanHotkey(bool),
    WifiHotkey(bool),
    DisplayOffHotkey(bool),
}

#[cfg(any(windows, test))]
fn menu_event(command: u32) -> Option<DesktopEvent> {
    match command {
        1 => Some(DesktopEvent::Show),
        2 => Some(DesktopEvent::FanMode(FanMode::Normal)),
        3 => Some(DesktopEvent::FanMode(FanMode::Quiet)),
        4 => Some(DesktopEvent::FanMode(FanMode::Gaming)),
        5 => Some(DesktopEvent::Exit),
        _ => None,
    }
}

#[cfg(any(windows, test))]
fn supported_fan_hid(vendor: u32, product: u32, usage_page: u16, usage: u16) -> bool {
    let (Ok(vendor), Ok(product)) = (u16::try_from(vendor), u16::try_from(product)) else {
        return false;
    };
    usage_page == 0xff02
        && usage == 1
        && aero_kbd::VENDOR_IDS.contains(&vendor)
        && aero_kbd::ITE_PRODUCT_IDS.contains(&product)
}

/// Recognize only documented four-byte fan/Wi-Fi/display-off reports. Other vendor reports
/// are neither retained nor forwarded. Count/bounds are checked before slicing.
#[cfg(any(windows, test))]
fn hotkey_reports(data: &[u8], report_size: u32, report_count: u32) -> Vec<DesktopEvent> {
    if report_size != 4 || report_count == 0 {
        return Vec::new();
    }
    let Some(bytes) = (report_count as usize).checked_mul(4) else {
        return Vec::new();
    };
    let Some(reports) = data.get(..bytes) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for report in reports.chunks_exact(4) {
        let event = match report {
            [4, 0, 0, 0x84] => DesktopEvent::FanHotkey,
            [4, 0, 0, 0x7c] => DesktopEvent::WifiHotkey,
            [4, 0, 0, 0x80] => DesktopEvent::DisplayOffHotkey,
            _ => continue,
        };
        if !events.contains(&event) {
            events.push(event);
        }
    }
    events
}

pub struct DesktopEvents {
    rx: mpsc::Receiver<DesktopEvent>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    worker: Option<JoinHandle<()>>,
    commands: mpsc::Sender<DesktopCommand>,
    tray_requested: Arc<AtomicBool>,
    tray_confirmed: Arc<AtomicBool>,
    mock: bool,
    hotkey_requested: Arc<AtomicBool>,
    hotkey_confirmed: Arc<AtomicBool>,
    wifi_requested: Arc<AtomicBool>,
    wifi_confirmed: Arc<AtomicBool>,
    display_off_requested: Arc<AtomicBool>,
    display_off_confirmed: Arc<AtomicBool>,
}

impl DesktopEvents {
    /// Startup is asynchronous, so native-window creation never stalls egui.
    /// Use `last_error` to report an unavailable notification listener.
    pub fn new(mock: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        let (commands, command_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let tray_requested = Arc::new(AtomicBool::new(false));
        let tray_confirmed = Arc::new(AtomicBool::new(false));
        let hotkey_requested = Arc::new(AtomicBool::new(false));
        let hotkey_confirmed = Arc::new(AtomicBool::new(false));
        let wifi_requested = Arc::new(AtomicBool::new(false));
        let wifi_confirmed = Arc::new(AtomicBool::new(false));
        let display_off_requested = Arc::new(AtomicBool::new(false));
        let display_off_confirmed = Arc::new(AtomicBool::new(false));
        let mut events = Self {
            rx,
            stop: stop.clone(),
            error: error.clone(),
            worker: None,
            commands,
            tray_requested,
            tray_confirmed: tray_confirmed.clone(),
            mock,
            hotkey_requested,
            hotkey_confirmed: hotkey_confirmed.clone(),
            wifi_requested,
            wifi_confirmed: wifi_confirmed.clone(),
            display_off_requested,
            display_off_confirmed: display_off_confirmed.clone(),
        };
        if mock {
            return events;
        }
        #[cfg(windows)]
        {
            let worker_error = error.clone();
            match std::thread::Builder::new()
                .name("aero-desktop-events".into())
                .spawn(move || {
                    if let Err(message) = native::run(
                        tx,
                        command_rx,
                        stop,
                        worker_error.clone(),
                        tray_confirmed.clone(),
                        hotkey_confirmed.clone(),
                        wifi_confirmed.clone(),
                        display_off_confirmed.clone(),
                    ) {
                        if let Ok(mut error) = worker_error.lock() {
                            *error = Some(message);
                        }
                    }
                    tray_confirmed.store(false, Ordering::Release);
                    hotkey_confirmed.store(false, Ordering::Release);
                    wifi_confirmed.store(false, Ordering::Release);
                    display_off_confirmed.store(false, Ordering::Release);
                }) {
                Ok(worker) => events.worker = Some(worker),
                Err(err) => {
                    if let Ok(mut error) = error.lock() {
                        *error = Some(format!("Could not start power notifications: {err}"));
                    }
                }
            }
        }
        #[cfg(not(windows))]
        {
            drop(tx);
            drop(command_rx);
            if let Ok(mut error) = error.lock() {
                *error = Some("Desktop power notifications require Windows.".into());
            }
        }
        events
    }

    pub fn poll(&self) -> Option<DesktopEvent> {
        self.rx.try_recv().ok()
    }

    pub fn last_error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|error| error.clone())
    }

    /// Requests a tray change without blocking the UI thread. Check
    /// `tray_enabled` before hiding the app: a queued request is not success.
    pub fn set_tray_enabled(&self, enabled: bool) {
        self.tray_requested.store(enabled, Ordering::Release);
        if self.mock {
            self.tray_confirmed.store(enabled, Ordering::Release);
            return;
        }
        if self.commands.send(DesktopCommand::Tray(enabled)).is_err() {
            self.tray_confirmed.store(false, Ordering::Release);
            if let Ok(mut error) = self.error.lock() {
                *error = Some("The desktop event listener is unavailable.".into());
            }
        }
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }

    /// True only after native icon registration succeeds and the user still
    /// requests it. Disabling invalidates confirmation immediately.
    pub fn tray_enabled(&self) -> bool {
        self.tray_requested.load(Ordering::Acquire) && self.tray_confirmed.load(Ordering::Acquire)
    }

    /// Mock mode has no real notification icon from which to reopen the app.
    pub fn can_hide_to_tray(&self) -> bool {
        !self.mock && self.tray_enabled()
    }

    /// Opts in to the known Gigabyte vendor-HID Fn fan report. No ordinary
    /// keyboard reports are inspected or logged.
    pub fn set_fan_hotkey_enabled(&self, enabled: bool) {
        self.hotkey_requested.store(enabled, Ordering::Release);
        if self.mock {
            self.hotkey_confirmed.store(enabled, Ordering::Release);
            return;
        }
        if self
            .commands
            .send(DesktopCommand::FanHotkey(enabled))
            .is_err()
        {
            self.hotkey_confirmed.store(false, Ordering::Release);
            if let Ok(mut error) = self.error.lock() {
                *error = Some("The desktop event listener is unavailable.".into());
            }
        }
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }

    pub fn fan_hotkey_enabled(&self) -> bool {
        self.hotkey_requested.load(Ordering::Acquire)
            && self.hotkey_confirmed.load(Ordering::Acquire)
    }

    pub fn set_wifi_hotkey_enabled(&self, enabled: bool) {
        self.wifi_requested.store(enabled, Ordering::Release);
        if self.mock {
            self.wifi_confirmed.store(enabled, Ordering::Release);
            return;
        }
        if self
            .commands
            .send(DesktopCommand::WifiHotkey(enabled))
            .is_err()
        {
            self.wifi_confirmed.store(false, Ordering::Release);
            if let Ok(mut error) = self.error.lock() {
                *error = Some("The desktop event listener is unavailable.".into());
            }
        }
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }

    pub fn wifi_hotkey_enabled(&self) -> bool {
        self.wifi_requested.load(Ordering::Acquire) && self.wifi_confirmed.load(Ordering::Acquire)
    }

    pub fn set_display_off_hotkey_enabled(&self, enabled: bool) {
        self.display_off_requested.store(enabled, Ordering::Release);
        if self.mock {
            self.display_off_confirmed.store(enabled, Ordering::Release);
            return;
        }
        if self
            .commands
            .send(DesktopCommand::DisplayOffHotkey(enabled))
            .is_err()
        {
            self.display_off_confirmed.store(false, Ordering::Release);
            if let Ok(mut error) = self.error.lock() {
                *error = Some("The desktop event listener is unavailable.".into());
            }
        }
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }

    pub fn display_off_hotkey_enabled(&self) -> bool {
        self.display_off_requested.load(Ordering::Acquire)
            && self.display_off_confirmed.load(Ordering::Acquire)
    }
}

impl Drop for DesktopEvents {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            // Unpark also covers the race where the worker has not parked yet.
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::cell::Cell;
    use std::sync::atomic::AtomicU64;
    use std::time::Duration;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::{
        GetRawInputData, GetRawInputDeviceInfoW, GetRawInputDeviceList, RegisterRawInputDevices,
        HRAWINPUT, RAWINPUTDEVICE, RAWINPUTDEVICELIST, RAWINPUTHEADER, RIDEV_DEVNOTIFY,
        RIDEV_INPUTSINK, RIDEV_REMOVE, RIDI_DEVICEINFO, RID_DEVICE_INFO, RID_HEADER, RID_INPUT,
        RIM_TYPEHID,
    };
    use windows::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE,
        NIM_SETVERSION, NINF_KEY, NIN_SELECT, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, ChangeWindowMessageFilterEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
        DestroyMenu, DestroyWindow, DispatchMessageW, EndMenu, GetCursorPos, GetWindowLongPtrW,
        KillTimer, LoadIconW, PeekMessageW, PostMessageW, PostQuitMessage, RegisterClassW,
        RegisterWindowMessageW, SetForegroundWindow, SetTimer, SetWindowLongPtrW, TrackPopupMenu,
        TranslateMessage, UnregisterClassW, CREATESTRUCTW, GWLP_USERDATA, HMENU, IDI_APPLICATION,
        MF_SEPARATOR, MF_STRING, MSG, MSGFLT_ALLOW, PBT_APMPOWERSTATUSCHANGE,
        PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PM_REMOVE, TPM_NONOTIFY, TPM_RETURNCMD,
        TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CONTEXTMENU, WM_DESTROY,
        WM_INPUT, WM_NCCREATE, WM_NCDESTROY, WM_NULL, WM_POWERBROADCAST, WM_QUIT, WM_TIMER,
        WNDCLASSW,
    };

    static NEXT_WINDOW_ID: AtomicU64 = AtomicU64::new(0);
    const TRAY_MESSAGE: u32 = WM_APP + 1;
    const TRAY_ID: u32 = 1;
    const MENU_STOP_TIMER: usize = 1;
    const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;

    struct WindowState {
        tx: mpsc::Sender<DesktopEvent>,
        stop: Arc<AtomicBool>,
        error: Arc<Mutex<Option<String>>>,
        tray_confirmed: Arc<AtomicBool>,
        tray_desired: Cell<bool>,
        tray_present: Cell<bool>,
        menu_open: Cell<bool>,
        taskbar_created: u32,
        taskbar_message_allowed: Cell<bool>,
        hotkey_confirmed: Arc<AtomicBool>,
        hotkey_desired: Cell<bool>,
        hotkey_registered: Cell<bool>,
        wifi_desired: Cell<bool>,
        wifi_confirmed: Arc<AtomicBool>,
        display_off_desired: Cell<bool>,
        display_off_confirmed: Arc<AtomicBool>,
    }

    impl WindowState {
        fn report_error(&self, message: String) {
            if let Ok(mut error) = self.error.lock() {
                *error = Some(message);
            }
        }

        fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
            NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: TRAY_ID,
                ..Default::default()
            }
        }

        fn remove_tray(&self, hwnd: HWND) {
            // Clear confirmation even if Explorer has already removed the icon.
            self.tray_confirmed.store(false, Ordering::Release);
            if self.tray_present.replace(false) {
                unsafe {
                    let _ = Shell_NotifyIconW(NIM_DELETE, &Self::icon_data(hwnd));
                }
            }
        }

        fn set_tray(&self, hwnd: HWND, enabled: bool) {
            self.tray_desired.set(enabled);
            if !enabled {
                self.remove_tray(hwnd);
                return;
            }
            if !self.taskbar_message_allowed.get() {
                self.report_error(
                    "Notification area recovery messages are unavailable; keep the app open."
                        .into(),
                );
                let _ = self.tx.send(DesktopEvent::Show);
                return;
            }
            if self.tray_present.get() {
                return;
            }
            let icon = match unsafe { LoadIconW(None, IDI_APPLICATION) } {
                Ok(icon) => icon,
                Err(err) => {
                    self.report_error(format!("Could not load the notification icon: {err}"));
                    return;
                }
            };
            // LoadIconW returns a shared system icon; it must not be destroyed.
            let mut data = Self::icon_data(hwnd);
            data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
            data.uCallbackMessage = TRAY_MESSAGE;
            data.hIcon = icon;
            for (slot, ch) in data.szTip.iter_mut().zip("aero-control".encode_utf16()) {
                *slot = ch;
            }
            if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
                self.report_error("Windows could not add the notification icon.".into());
                return;
            }
            self.tray_present.set(true);
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            if !unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) }.as_bool() {
                self.remove_tray(hwnd);
                self.report_error("Windows could not enable notification icon actions.".into());
                return;
            }
            self.tray_confirmed.store(true, Ordering::Release);
            if let Ok(mut error) = self.error.lock() {
                *error = None;
            }
        }

        fn set_hotkey(&self, hwnd: HWND, enabled: bool) {
            self.hotkey_desired.set(enabled);
            self.update_hotkeys(hwnd);
        }

        fn set_wifi_hotkey(&self, hwnd: HWND, enabled: bool) {
            self.wifi_desired.set(enabled);
            self.update_hotkeys(hwnd);
        }

        fn set_display_off_hotkey(&self, hwnd: HWND, enabled: bool) {
            self.display_off_desired.set(enabled);
            self.update_hotkeys(hwnd);
        }

        fn update_hotkeys(&self, hwnd: HWND) {
            let enabled = self.hotkey_desired.get()
                || self.wifi_desired.get()
                || self.display_off_desired.get();
            self.hotkey_confirmed.store(false, Ordering::Release);
            self.wifi_confirmed.store(false, Ordering::Release);
            self.display_off_confirmed.store(false, Ordering::Release);
            if enabled {
                match known_fan_device_present() {
                    Ok(true) => {}
                    Ok(false) => {
                        self.report_error(
                            "A supported Gigabyte Fn keyboard collection was not found.".into(),
                        );
                        return;
                    }
                    Err(error) => {
                        self.report_error(error);
                        return;
                    }
                }
            }
            if !enabled && !self.hotkey_registered.get() {
                return;
            }
            let registration = RAWINPUTDEVICE {
                usUsagePage: 0xff02,
                usUsage: 1,
                dwFlags: if enabled {
                    RIDEV_INPUTSINK | RIDEV_DEVNOTIFY
                } else {
                    RIDEV_REMOVE
                },
                hwndTarget: if enabled { hwnd } else { HWND::default() },
            };
            match unsafe {
                RegisterRawInputDevices(
                    &[registration],
                    std::mem::size_of::<RAWINPUTDEVICE>() as u32,
                )
            } {
                Ok(()) => {
                    self.hotkey_registered.set(enabled);
                    self.hotkey_confirmed
                        .store(enabled && self.hotkey_desired.get(), Ordering::Release);
                    self.wifi_confirmed
                        .store(enabled && self.wifi_desired.get(), Ordering::Release);
                    self.display_off_confirmed
                        .store(enabled && self.display_off_desired.get(), Ordering::Release);
                    if let Ok(mut error) = self.error.lock() {
                        *error = None;
                    }
                }
                Err(err) => self.report_error(format!("Fn hotkey registration failed: {err}")),
            }
        }

        fn show_menu(&self, hwnd: HWND, x: i32, y: i32) -> Result<(), String> {
            if self.stop.load(Ordering::Acquire) || self.menu_open.replace(true) {
                return Ok(());
            }
            let result = self.track_menu(hwnd, x, y);
            self.menu_open.set(false);
            result
        }

        fn track_menu(&self, hwnd: HWND, x: i32, y: i32) -> Result<(), String> {
            let menu = Menu(unsafe { CreatePopupMenu() }.map_err(|err| err.to_string())?);
            unsafe {
                AppendMenuW(menu.0, MF_STRING, 1, w!("Open aero-control"))
                    .map_err(|err| err.to_string())?;
                AppendMenuW(menu.0, MF_SEPARATOR, 0, PCWSTR::null())
                    .map_err(|err| err.to_string())?;
                AppendMenuW(menu.0, MF_STRING, 2, w!("Normal")).map_err(|err| err.to_string())?;
                AppendMenuW(menu.0, MF_STRING, 3, w!("Quiet")).map_err(|err| err.to_string())?;
                AppendMenuW(menu.0, MF_STRING, 4, w!("Gaming")).map_err(|err| err.to_string())?;
                AppendMenuW(menu.0, MF_SEPARATOR, 0, PCWSTR::null())
                    .map_err(|err| err.to_string())?;
                AppendMenuW(menu.0, MF_STRING, 5, w!("Exit")).map_err(|err| err.to_string())?;
            }
            let mut at = POINT { x, y };
            if x == -1 && y == -1 {
                unsafe { GetCursorPos(&mut at) }.map_err(|err| err.to_string())?;
            }
            // TrackPopupMenu has a modal loop. Its owner timer ensures shutdown
            // cancels that loop, so Drop cannot wait for a user to dismiss it.
            let timer = unsafe { SetTimer(Some(hwnd), MENU_STOP_TIMER, 50, None) };
            if timer == 0 {
                return Err("Could not prepare the notification menu.".into());
            }
            let _timer = Timer(hwnd, timer);
            let command = unsafe {
                let _ = SetForegroundWindow(hwnd);
                TrackPopupMenu(
                    menu.0,
                    TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
                    at.x,
                    at.y,
                    None,
                    hwnd,
                    None,
                )
                .0 as u32
            };
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            }
            if !self.stop.load(Ordering::Acquire) {
                if let Some(event) = menu_event(command) {
                    let _ = self.tx.send(event);
                }
            }
            Ok(())
        }
    }

    struct Menu(HMENU);

    impl Drop for Menu {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyMenu(self.0);
            }
        }
    }

    struct Timer(HWND, usize);

    impl Drop for Timer {
        fn drop(&mut self) {
            unsafe {
                let _ = KillTimer(Some(self.0), self.1);
            }
        }
    }

    fn supported_device(handle: HANDLE) -> bool {
        if handle.is_invalid() {
            return false;
        }
        let mut info = RID_DEVICE_INFO {
            cbSize: std::mem::size_of::<RID_DEVICE_INFO>() as u32,
            ..Default::default()
        };
        let mut size = info.cbSize;
        let copied = unsafe {
            GetRawInputDeviceInfoW(
                Some(handle),
                RIDI_DEVICEINFO,
                Some((&mut info as *mut RID_DEVICE_INFO).cast()),
                &mut size,
            )
        };
        if copied == u32::MAX
            || copied < std::mem::size_of::<RID_DEVICE_INFO>() as u32
            || info.dwType != RIM_TYPEHID
        {
            return false;
        }
        let hid = unsafe { info.Anonymous.hid };
        supported_fan_hid(
            hid.dwVendorId,
            hid.dwProductId,
            hid.usUsagePage,
            hid.usUsage,
        )
    }

    fn known_fan_device_present() -> Result<bool, String> {
        let item_size = std::mem::size_of::<RAWINPUTDEVICELIST>() as u32;
        // The list can change between sizing and reading when USB reconnects.
        for _ in 0..2 {
            let mut count = 0;
            if unsafe { GetRawInputDeviceList(None, &mut count, item_size) } == u32::MAX {
                return Err(format!(
                    "Could not enumerate Fn input devices: {}",
                    windows::core::Error::from_win32()
                ));
            }
            if count == 0 {
                return Ok(false);
            }
            if count > 4096 {
                return Err("The raw-input device list is unexpectedly large.".into());
            }
            let mut devices = vec![RAWINPUTDEVICELIST::default(); count as usize];
            let received =
                unsafe { GetRawInputDeviceList(Some(devices.as_mut_ptr()), &mut count, item_size) };
            if received == u32::MAX {
                continue;
            }
            if received as usize > devices.len() {
                return Err("The raw-input device list changed while reading it.".into());
            }
            return Ok(devices[..received as usize]
                .iter()
                .any(|device| device.dwType == RIM_TYPEHID && supported_device(device.hDevice)));
        }
        Err("Could not read a stable Fn input device list; try enabling it again.".into())
    }

    fn raw_hotkey_reports(input: HRAWINPUT) -> Vec<DesktopEvent> {
        let header_size = std::mem::size_of::<RAWINPUTHEADER>();
        let mut header = RAWINPUTHEADER::default();
        let mut size = header_size as u32;
        let copied = unsafe {
            GetRawInputData(
                input,
                RID_HEADER,
                Some((&mut header as *mut RAWINPUTHEADER).cast()),
                &mut size,
                header_size as u32,
            )
        };
        if copied != header_size as u32
            || header.dwType != RIM_TYPEHID.0
            || !supported_device(header.hDevice)
        {
            return Vec::new();
        }
        let capacity = header.dwSize as usize;
        if capacity < header_size + 8 || capacity > 64 * 1024 {
            return Vec::new();
        }
        // Raw input requires DWORD-aligned storage. Vec<usize> supplies that
        // alignment on both x86 and x64; header reads still use read_unaligned.
        let words = capacity.div_ceil(std::mem::size_of::<usize>());
        let mut buffer = vec![0usize; words];
        size = capacity as u32;
        let copied = unsafe {
            GetRawInputData(
                input,
                RID_INPUT,
                Some(buffer.as_mut_ptr().cast()),
                &mut size,
                header_size as u32,
            )
        };
        if copied == u32::MAX || copied as usize > capacity || (copied as usize) < header_size + 8 {
            return Vec::new();
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), copied as usize) };
        let full_header =
            unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<RAWINPUTHEADER>()) };
        let declared = full_header.dwSize as usize;
        if full_header.dwType != RIM_TYPEHID.0
            || full_header.hDevice != header.hDevice
            || declared > bytes.len()
            || declared < header_size + 8
        {
            return Vec::new();
        }
        let report_size =
            u32::from_le_bytes(bytes[header_size..header_size + 4].try_into().unwrap());
        let report_count =
            u32::from_le_bytes(bytes[header_size + 4..header_size + 8].try_into().unwrap());
        hotkey_reports(&bytes[header_size + 8..declared], report_size, report_count)
    }

    struct WindowClass {
        name: Vec<u16>,
        instance: HINSTANCE,
    }

    impl Drop for WindowClass {
        fn drop(&mut self) {
            unsafe {
                let _ = UnregisterClassW(PCWSTR(self.name.as_ptr()), Some(self.instance));
            }
        }
    }

    struct Window(HWND);

    impl Drop for Window {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }

    pub fn run(
        tx: mpsc::Sender<DesktopEvent>,
        commands: mpsc::Receiver<DesktopCommand>,
        stop: Arc<AtomicBool>,
        error: Arc<Mutex<Option<String>>>,
        tray_confirmed: Arc<AtomicBool>,
        hotkey_confirmed: Arc<AtomicBool>,
        wifi_confirmed: Arc<AtomicBool>,
        display_off_confirmed: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let instance = HINSTANCE(
            unsafe { GetModuleHandleW(None) }
                .map_err(|err| format!("Power listener module lookup failed: {err}"))?
                .0,
        );
        let id = NEXT_WINDOW_ID.fetch_add(1, Ordering::Relaxed);
        let name: Vec<u16> = format!("AeroControlPowerEvents_{}_{}", std::process::id(), id)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: PCWSTR(name.as_ptr()),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err(format!(
                "Power listener class registration failed: {}",
                windows::core::Error::from_win32()
            ));
        }
        let class = WindowClass { name, instance };
        // This owner outlives the native window, including WM_NCDESTROY. The
        // pointer is only read by callbacks running on this same worker thread.
        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        if taskbar_created == 0 {
            return Err("Could not register Explorer recovery notifications.".into());
        }
        let state = Box::new(WindowState {
            tx,
            stop: stop.clone(),
            error,
            tray_confirmed,
            tray_desired: Cell::new(false),
            tray_present: Cell::new(false),
            menu_open: Cell::new(false),
            taskbar_created,
            taskbar_message_allowed: Cell::new(false),
            hotkey_confirmed,
            hotkey_desired: Cell::new(false),
            hotkey_registered: Cell::new(false),
            wifi_desired: Cell::new(false),
            wifi_confirmed,
            display_off_desired: Cell::new(false),
            display_off_confirmed,
        });
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                PCWSTR(class.name.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                Some((&*state as *const WindowState).cast()),
            )
        }
        .map_err(|err| format!("Power listener window creation failed: {err}"))?;
        // A hidden top-level window receives broadcasts. A message-only window
        // would not receive WM_POWERBROADCAST.
        let _window = Window(hwnd);
        // Explorer normally has lower integrity than this elevated app. Allow
        // only this registered recovery broadcast, not an entire message range.
        match unsafe { ChangeWindowMessageFilterEx(hwnd, taskbar_created, MSGFLT_ALLOW, None) } {
            Ok(()) => state.taskbar_message_allowed.set(true),
            Err(err) => {
                state.report_error(format!(
                    "Could not allow Explorer recovery notifications: {err}"
                ));
                let _ = state.tx.send(DesktopEvent::Show);
            }
        }
        let mut message = MSG::default();
        while !stop.load(Ordering::Acquire) {
            while let Ok(command) = commands.try_recv() {
                match command {
                    DesktopCommand::Tray(enabled) => state.set_tray(hwnd, enabled),
                    DesktopCommand::FanHotkey(enabled) => state.set_hotkey(hwnd, enabled),
                    DesktopCommand::WifiHotkey(enabled) => state.set_wifi_hotkey(hwnd, enabled),
                    DesktopCommand::DisplayOffHotkey(enabled) => {
                        state.set_display_off_hotkey(hwnd, enabled)
                    }
                }
            }
            while !stop.load(Ordering::Acquire)
                && unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool()
            {
                if message.message == WM_QUIT {
                    return Ok(());
                }
                unsafe {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            // A short worker-only wait keeps native broadcasts responsive and
            // lets Drop wake the thread without posting to a stale HWND.
            std::thread::park_timeout(Duration::from_millis(50));
        }
        Ok(())
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_NCCREATE {
            let creation = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, creation.lpCreateParams as isize);
        }
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const WindowState;
        if let Some(state) = state.as_ref() {
            if message == WM_INPUT && state.hotkey_registered.get() {
                // Each action is independently opted in and confirmed. Batch
                // duplicates become one action; no payload is saved or logged.
                for event in raw_hotkey_reports(HRAWINPUT(lparam.0 as *mut _)) {
                    let enabled = match event {
                        DesktopEvent::FanHotkey => {
                            state.hotkey_desired.get()
                                && state.hotkey_confirmed.load(Ordering::Acquire)
                        }
                        DesktopEvent::WifiHotkey => {
                            state.wifi_desired.get() && state.wifi_confirmed.load(Ordering::Acquire)
                        }
                        DesktopEvent::DisplayOffHotkey => {
                            state.display_off_desired.get()
                                && state.display_off_confirmed.load(Ordering::Acquire)
                        }
                        _ => false,
                    };
                    if enabled {
                        let _ = state.tx.send(event);
                    }
                }
            }
            if message == state.taskbar_created {
                // Explorer lost its icon table; don't claim the old icon exists.
                state.tray_present.set(false);
                state.tray_confirmed.store(false, Ordering::Release);
                if state.tray_desired.get() {
                    state.set_tray(hwnd, true);
                    if !state.tray_confirmed.load(Ordering::Acquire) {
                        let _ = state.tx.send(DesktopEvent::Show);
                    }
                }
                return LRESULT(0);
            }
            if message == TRAY_MESSAGE {
                // Version 4 puts the notification in LOWORD(lParam), icon id
                // in HIWORD(lParam), and signed screen coordinates in wParam.
                let icon_id = ((lparam.0 as usize >> 16) & 0xffff) as u32;
                if icon_id != TRAY_ID || !state.tray_present.get() {
                    return LRESULT(0);
                }
                match (lparam.0 as u32) & 0xffff {
                    NIN_SELECT | NIN_KEYSELECT => {
                        let _ = state.tx.send(DesktopEvent::Show);
                    }
                    WM_CONTEXTMENU => {
                        let x = (wparam.0 as u16) as i16 as i32;
                        let y = ((wparam.0 >> 16) as u16) as i16 as i32;
                        if let Err(error) = state.show_menu(hwnd, x, y) {
                            state.report_error(format!("Notification menu failed: {error}"));
                        }
                    }
                    _ => {}
                }
                return LRESULT(0);
            }
            if message == WM_TIMER && wparam.0 == MENU_STOP_TIMER {
                if state.stop.load(Ordering::Acquire) && state.menu_open.get() {
                    let _ = EndMenu();
                }
                return LRESULT(0);
            }
            if message == WM_DESTROY {
                state.tray_desired.set(false);
                state.remove_tray(hwnd);
                state.wifi_desired.set(false);
                state.display_off_desired.set(false);
                state.set_hotkey(hwnd, false);
            }
        }
        if message == WM_POWERBROADCAST {
            let event = match wparam.0 as u32 {
                PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => Some(DesktopEvent::Resume),
                PBT_APMPOWERSTATUSCHANGE => Some(DesktopEvent::PowerChanged),
                _ => None,
            };
            if let (Some(event), Some(state)) = (event, state.as_ref()) {
                let _ = state.tx.send(event);
            }
            return LRESULT(1);
        }
        if message == WM_NCDESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        }
        if message == WM_DESTROY {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_does_not_create_a_worker_or_events() {
        let events = DesktopEvents::new(true);
        assert!(events.worker.is_none());
        assert!(events.poll().is_none());
        assert!(events.last_error().is_none());
        assert!(!events.tray_enabled());
        assert!(!events.can_hide_to_tray());
        assert!(!events.fan_hotkey_enabled());
        assert!(!events.wifi_hotkey_enabled());
        assert!(!events.display_off_hotkey_enabled());
    }

    #[test]
    fn mock_tray_is_opt_in_and_stays_in_memory() {
        let events = DesktopEvents::new(true);
        events.set_tray_enabled(true);
        assert!(events.tray_enabled());
        assert!(!events.can_hide_to_tray());
        assert!(events.worker.is_none());
        assert!(events.poll().is_none());
        events.set_tray_enabled(false);
        assert!(!events.tray_enabled());
        assert!(events.last_error().is_none());
    }

    #[test]
    fn a_requested_icon_is_not_confirmed_until_registration_succeeds() {
        let events = DesktopEvents::new(true);
        events.tray_requested.store(true, Ordering::Release);
        assert!(!events.tray_enabled());
        events.tray_confirmed.store(true, Ordering::Release);
        assert!(events.tray_enabled());
        // A late enable acknowledgement cannot override a newer disable.
        events.tray_requested.store(false, Ordering::Release);
        assert!(!events.tray_enabled());
    }

    #[test]
    fn tray_routes_only_supported_unarmed_fan_actions() {
        assert_eq!(menu_event(1), Some(DesktopEvent::Show));
        assert_eq!(menu_event(2), Some(DesktopEvent::FanMode(FanMode::Normal)));
        assert_eq!(menu_event(3), Some(DesktopEvent::FanMode(FanMode::Quiet)));
        assert_eq!(menu_event(4), Some(DesktopEvent::FanMode(FanMode::Gaming)));
        assert_eq!(menu_event(5), Some(DesktopEvent::Exit));
        assert_eq!(menu_event(0), None); // Cancel.
        assert_eq!(menu_event(6), None); // No unconfirmed Max/Fixed/Custom action.
    }

    #[test]
    fn mock_fan_hotkey_is_opt_in_without_native_registration() {
        let events = DesktopEvents::new(true);
        events.set_fan_hotkey_enabled(true);
        assert!(events.fan_hotkey_enabled());
        assert!(events.worker.is_none());
        assert!(events.poll().is_none());
        events.set_fan_hotkey_enabled(false);
        assert!(!events.fan_hotkey_enabled());
    }

    #[test]
    fn hotkey_request_requires_registration_confirmation() {
        let events = DesktopEvents::new(true);
        events.hotkey_requested.store(true, Ordering::Release);
        assert!(!events.fan_hotkey_enabled());
        events.hotkey_confirmed.store(true, Ordering::Release);
        assert!(events.fan_hotkey_enabled());
        events.hotkey_requested.store(false, Ordering::Release);
        assert!(!events.fan_hotkey_enabled());
    }

    #[test]
    fn wifi_hotkey_is_independent_and_requires_confirmation() {
        let events = DesktopEvents::new(true);
        events.wifi_requested.store(true, Ordering::Release);
        assert!(!events.wifi_hotkey_enabled());
        events.wifi_confirmed.store(true, Ordering::Release);
        assert!(events.wifi_hotkey_enabled());
        assert!(!events.fan_hotkey_enabled());
        events.set_fan_hotkey_enabled(true);
        events.set_wifi_hotkey_enabled(false);
        assert!(events.fan_hotkey_enabled());
        assert!(!events.wifi_hotkey_enabled());
        // A late native acknowledgement must not re-enable a disabled key.
        events.wifi_confirmed.store(true, Ordering::Release);
        assert!(!events.wifi_hotkey_enabled());
        assert!(events.worker.is_none());
    }

    #[test]
    fn display_off_is_explicit_and_independent_of_the_other_hotkeys() {
        let events = DesktopEvents::new(true);
        events.display_off_requested.store(true, Ordering::Release);
        assert!(!events.display_off_hotkey_enabled());
        events.display_off_confirmed.store(true, Ordering::Release);
        assert!(events.display_off_hotkey_enabled());
        assert!(!events.fan_hotkey_enabled());
        assert!(!events.wifi_hotkey_enabled());
        events.set_wifi_hotkey_enabled(true);
        events.set_display_off_hotkey_enabled(false);
        assert!(events.wifi_hotkey_enabled());
        assert!(!events.display_off_hotkey_enabled());
        events.display_off_confirmed.store(true, Ordering::Release);
        assert!(!events.display_off_hotkey_enabled());
        assert!(events.worker.is_none());
    }

    #[test]
    fn display_off_report_is_exact_and_deduplicated() {
        assert_eq!(
            hotkey_reports(&[4, 0, 0, 0x80, 4, 0, 0, 0x80], 4, 2),
            [DesktopEvent::DisplayOffHotkey]
        );
        assert_eq!(
            hotkey_reports(&[4, 0, 0, 0x7c, 4, 0, 0, 0x80, 4, 0, 0, 0x84], 4, 3),
            [
                DesktopEvent::WifiHotkey,
                DesktopEvent::DisplayOffHotkey,
                DesktopEvent::FanHotkey
            ]
        );
        assert!(hotkey_reports(&[4, 0, 1, 0x80], 4, 1).is_empty());
        assert!(hotkey_reports(&[4, 0, 0, 0x80], 5, 1).is_empty());
    }

    #[test]
    fn unrelated_devices_and_usage_collections_are_rejected() {
        assert!(supported_fan_hid(0x1044, 0x7a3f, 0xff02, 1));
        assert!(supported_fan_hid(0x04d9, 0x7a39, 0xff02, 1));
        assert!(!supported_fan_hid(0x1234, 0x7a3f, 0xff02, 1));
        assert!(!supported_fan_hid(0x1044, 0xffff, 0xff02, 1));
        assert!(!supported_fan_hid(0x1044, 0x7a3f, 7, 6));
        assert!(!supported_fan_hid(0x1044, 0x7a3f, 0xff02, 2));
        assert!(!supported_fan_hid(0x11044, 0x7a3f, 0xff02, 1));
    }

    #[test]
    fn batched_reports_deduplicate_each_supported_action() {
        let reports = [
            4, 0, 0, 0x83, 4, 0, 0, 0x84, 4, 0, 0, 0x84, 4, 0, 0, 0x7c, 4, 0, 0, 0x7c,
        ];
        assert_eq!(
            hotkey_reports(&reports, 4, 5),
            [DesktopEvent::FanHotkey, DesktopEvent::WifiHotkey]
        );
        assert!(hotkey_reports(&reports, 4, 1).is_empty());
        assert_eq!(hotkey_reports(&reports, 4, 2), [DesktopEvent::FanHotkey]);
        assert!(hotkey_reports(&reports, 3, 4).is_empty());
        assert!(hotkey_reports(&reports, 4, 0).is_empty());
        assert!(hotkey_reports(&reports, 4, 6).is_empty());
        assert!(hotkey_reports(&reports, 4, u32::MAX).is_empty());
        assert!(hotkey_reports(&[4, 0, 0], 4, 1).is_empty());
        // OS/firmware lighting and touchpad reports are never forwarded.
        assert!(hotkey_reports(&[4, 1, 25, 0, 4, 0, 0, 0x81], 4, 2).is_empty());
    }
}
