//! Windows controls also exposed by Control Center's SmartManager.
//!
//! Brightness follows CPOWERAPI.GetBrightness/SetBrightness: the active power
//! plan's brightness setting for the current AC/DC source. It is not a measured
//! panel luminance value. Volume uses the default multimedia render endpoint.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Volume {
    pub percent: u8,
    pub muted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Brightness {
    pub percent: u8,
    pub on_ac: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsPage {
    Display,
    Sound,
    Network,
    Bluetooth,
    Touchpad,
    Projection,
    AdvancedColour,
}

impl SettingsPage {
    pub const ALL: [Self; 7] = [
        Self::Display,
        Self::Sound,
        Self::Network,
        Self::Bluetooth,
        Self::Touchpad,
        Self::Projection,
        Self::AdvancedColour,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Display => "Open display settings",
            Self::Sound => "Open sound settings",
            Self::Network => "Open network settings",
            Self::Bluetooth => "Open Bluetooth settings",
            Self::Touchpad => "Open touchpad settings",
            Self::Projection => "Open projection settings",
            Self::AdvancedColour => "Open HDR and colour settings",
        }
    }

    #[cfg(windows)]
    fn uri(self) -> &'static str {
        match self {
            Self::Display => "ms-settings:display",
            Self::Sound => "ms-settings:sound",
            Self::Network => "ms-settings:network-status",
            Self::Bluetooth => "ms-settings:bluetooth",
            Self::Touchpad => "ms-settings:devices-touchpad",
            Self::Projection => "ms-settings:project",
            Self::AdvancedColour => "ms-settings:display-advancedcolor",
        }
    }
}

pub enum SystemControls {
    Mock {
        volume: Volume,
        brightness: Brightness,
    },
    #[cfg(windows)]
    Real,
}

impl SystemControls {
    pub fn new(mock: bool) -> Self {
        #[cfg(windows)]
        if !mock {
            return Self::Real;
        }
        let _ = mock;
        Self::Mock {
            volume: Volume {
                percent: 50,
                muted: false,
            },
            brightness: Brightness {
                percent: 60,
                on_ac: true,
            },
        }
    }

    pub fn is_mock(&self) -> bool {
        matches!(self, Self::Mock { .. })
    }

    pub fn volume(&self) -> Result<Volume, String> {
        match self {
            Self::Mock { volume, .. } => Ok(*volume),
            #[cfg(windows)]
            Self::Real => real::volume(),
        }
    }

    pub fn set_volume(&mut self, percent: u8) -> Result<Volume, String> {
        check_percent(percent)?;
        match self {
            Self::Mock { volume, .. } => {
                volume.percent = percent;
                volume.muted = percent == 0;
                Ok(*volume)
            }
            #[cfg(windows)]
            Self::Real => real::set_volume(percent),
        }
    }

    pub fn set_mute(&mut self, muted: bool) -> Result<Volume, String> {
        match self {
            Self::Mock { volume, .. } => {
                volume.muted = muted;
                Ok(*volume)
            }
            #[cfg(windows)]
            Self::Real => real::set_mute(muted),
        }
    }

    pub fn brightness(&self) -> Result<Brightness, String> {
        match self {
            Self::Mock { brightness, .. } => Ok(*brightness),
            #[cfg(windows)]
            Self::Real => real::brightness(),
        }
    }

    pub fn set_brightness(&mut self, percent: u8) -> Result<Brightness, String> {
        check_percent(percent)?;
        match self {
            Self::Mock { brightness, .. } => {
                brightness.percent = percent;
                Ok(*brightness)
            }
            #[cfg(windows)]
            Self::Real => real::set_brightness(percent),
        }
    }

    pub fn open_settings(&self, page: SettingsPage) -> Result<(), String> {
        match self {
            Self::Mock { .. } => Ok(()),
            #[cfg(windows)]
            Self::Real => real::open_settings(page),
        }
    }
}

fn check_percent(percent: u8) -> Result<(), String> {
    if percent <= 100 {
        Ok(())
    } else {
        Err("The value must be between 0 and 100%.".into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WirelessKind {
    WiFi,
    Bluetooth,
}

impl WirelessKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::WiFi => "Wi-Fi",
            Self::Bluetooth => "Bluetooth",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WirelessState {
    On,
    Off,
    Disabled,
    Unknown,
}

impl WirelessState {
    pub fn label(self) -> &'static str {
        match self {
            Self::On => "On",
            Self::Off => "Off",
            Self::Disabled => "Disabled by hardware or system",
            Self::Unknown => "State unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WirelessRadio {
    pub name: String,
    pub kind: WirelessKind,
    pub state: WirelessState,
}

/// Decide from a freshly read group. An indeterminate adapter prevents a
/// group toggle rather than guessing from a stale display value.
fn wifi_toggle_target(radios: &[WirelessRadio]) -> Result<bool, String> {
    let wifi: Vec<_> = radios
        .iter()
        .filter(|radio| radio.kind == WirelessKind::WiFi)
        .collect();
    if wifi.is_empty() {
        return Err("No Wi-Fi adapter is available.".into());
    }
    if wifi.iter().any(|radio| {
        matches!(
            radio.state,
            WirelessState::Disabled | WirelessState::Unknown
        )
    }) {
        return Err("A Wi-Fi adapter has an unknown state or is disabled by hardware or Windows; no adapters were changed.".into());
    }
    Ok(!wifi.iter().any(|radio| radio.state == WirelessState::On))
}

fn verify_wifi_toggle(
    radios: &[WirelessRadio],
    expected_names: &[String],
    enabled: bool,
) -> Result<(), String> {
    let wifi: Vec<_> = radios
        .iter()
        .filter(|radio| radio.kind == WirelessKind::WiFi)
        .collect();
    let mut names: Vec<_> = wifi.iter().map(|radio| radio.name.as_str()).collect();
    let mut expected: Vec<_> = expected_names.iter().map(String::as_str).collect();
    names.sort_unstable();
    expected.sort_unstable();
    if names != expected || wifi.is_empty() {
        return Err("The Wi-Fi adapter list changed during the request. Refresh to check the current state.".into());
    }
    let target = if enabled {
        WirelessState::On
    } else {
        WirelessState::Off
    };
    if wifi.iter().any(|radio| radio.state != target) {
        return Err("The driver has not confirmed the requested state for every Wi-Fi adapter. Some adapters may already have changed; refresh to check.".into());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RadioAccess {
    Allowed,
    DeniedByUser,
    DeniedBySystem,
    Unspecified,
}

impl RadioAccess {
    fn check(self) -> Result<(), String> {
        match self {
            Self::Allowed => Ok(()),
            Self::DeniedByUser => Err("Radio control permission was denied. Allow access in Windows, then retry radio access.".into()),
            Self::DeniedBySystem => Err("Windows policy or the radio driver denies radio control.".into()),
            Self::Unspecified => Err("Windows did not grant radio control access.".into()),
        }
    }
}

struct RadioEvent {
    radios: Result<Vec<WirelessRadio>, String>,
    status: Option<(String, bool)>,
    access: Option<RadioAccess>,
}

/// Radio operations run off the UI thread. Permission is initiated by an
/// explicit toggle click or opted-in Fn action on the UI thread and cached for
/// this app session. There is no periodic radio enumeration.
pub struct WirelessControls {
    mock: bool,
    pub radios: Result<Vec<WirelessRadio>, String>,
    receiver: Option<std::sync::mpsc::Receiver<RadioEvent>>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    access: Option<RadioAccess>,
    #[cfg(windows)]
    ui_runtime: Option<radio_real::Runtime>,
}

impl WirelessControls {
    pub fn new(mock: bool) -> Self {
        let mock = mock || !cfg!(windows);
        let radios = if mock {
            Ok(vec![
                WirelessRadio {
                    name: "Mock Wi-Fi adapter".into(),
                    kind: WirelessKind::WiFi,
                    state: WirelessState::On,
                },
                WirelessRadio {
                    name: "Mock Bluetooth adapter".into(),
                    kind: WirelessKind::Bluetooth,
                    state: WirelessState::On,
                },
            ])
        } else {
            Err("Radio state has not been read yet.".into())
        };
        Self {
            mock,
            radios,
            receiver: None,
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            access: None,
            #[cfg(windows)]
            ui_runtime: None,
        }
    }

    pub fn busy(&self) -> bool {
        self.receiver.is_some()
    }

    pub fn retry_access(&mut self) {
        if !self.busy() {
            self.access = None;
        }
    }

    pub fn refresh(&mut self) {
        if self.mock || self.busy() {
            return;
        }
        #[cfg(windows)]
        {
            let cancel = self.cancel.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            self.receiver = Some(rx);
            std::thread::spawn(move || {
                let result = radio_real::read(&cancel);
                let _ = tx.send(RadioEvent {
                    radios: result,
                    status: None,
                    access: None,
                });
            });
        }
    }

    pub fn poll(&mut self) -> Option<(String, bool)> {
        let receiver = self.receiver.as_ref()?;
        match receiver.try_recv() {
            Ok(event) => {
                self.receiver = None;
                self.radios = event.radios;
                if let Some(access) = event.access {
                    self.access = Some(access);
                }
                event.status
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                self.radios = Err("The radio operation ended unexpectedly.".into());
                Some(("The radio operation ended unexpectedly.".into(), true))
            }
        }
    }

    pub fn set(&mut self, selected: WirelessRadio, enabled: bool) -> Result<(), String> {
        if self.busy() {
            return Err("A radio operation is already running.".into());
        }
        if matches!(
            selected.state,
            WirelessState::Disabled | WirelessState::Unknown
        ) {
            return Err("This radio cannot currently be changed by software.".into());
        }
        if let Some(access) = self.access {
            access.check()?;
        }
        if self.mock {
            let radios = self.radios.as_mut().map_err(|error| error.clone())?;
            let radio = radios
                .iter_mut()
                .find(|radio| radio.kind == selected.kind && radio.name == selected.name)
                .ok_or("The selected radio is no longer available.")?;
            radio.state = if enabled {
                WirelessState::On
            } else {
                WirelessState::Off
            };
            return Ok(());
        }
        #[cfg(windows)]
        {
            if self.ui_runtime.is_none() {
                self.ui_runtime = Some(radio_real::Runtime::ui()?);
            }
            // This call only initiates permission UI; waiting happens in the worker.
            let permission = if self.access.is_none() {
                Some(radio_real::request_access()?)
            } else {
                None
            };
            let access = self.access;
            let cancel = self.cancel.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            self.receiver = Some(rx);
            std::thread::spawn(move || {
                let event = radio_real::set(selected, enabled, permission, access, &cancel);
                let _ = tx.send(event);
            });
        }
        Ok(())
    }

    /// Toggle only Wi-Fi, using a new Windows snapshot inside the worker. The
    /// displayed adapter cache is never used to choose the requested state.
    pub fn toggle_wifi(&mut self) -> Result<(), String> {
        if self.busy() {
            return Err("A radio operation is already running; wait before pressing the Wi-Fi Fn key again.".into());
        }
        if let Some(access) = self.access {
            access.check()?;
        }
        if self.mock {
            let radios = self.radios.as_mut().map_err(|error| error.clone())?;
            let enabled = wifi_toggle_target(radios)?;
            let names = radios
                .iter()
                .filter(|radio| radio.kind == WirelessKind::WiFi)
                .map(|radio| radio.name.clone())
                .collect::<Vec<_>>();
            for radio in radios
                .iter_mut()
                .filter(|radio| radio.kind == WirelessKind::WiFi)
            {
                radio.state = if enabled {
                    WirelessState::On
                } else {
                    WirelessState::Off
                };
            }
            return verify_wifi_toggle(radios, &names, enabled);
        }
        #[cfg(windows)]
        {
            if self.ui_runtime.is_none() {
                self.ui_runtime = Some(radio_real::Runtime::ui()?);
            }
            let permission = if self.access.is_none() {
                Some(radio_real::request_access()?)
            } else {
                None
            };
            let access = self.access;
            let cancel = self.cancel.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            self.receiver = Some(rx);
            std::thread::spawn(move || {
                let _ = tx.send(radio_real::toggle_wifi(permission, access, &cancel));
            });
        }
        Ok(())
    }
}

impl Drop for WirelessControls {
    fn drop(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(windows)]
mod radio_real {
    use super::{
        verify_wifi_toggle, wifi_toggle_target, RadioAccess, RadioEvent, WirelessKind,
        WirelessRadio, WirelessState,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    use windows::core::RuntimeType;
    use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};
    use windows::Foundation::{AsyncStatus, IAsyncOperation};
    use windows::Win32::System::WinRT::{
        RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED, RO_INIT_SINGLETHREADED,
    };

    pub struct Runtime(bool);
    impl Runtime {
        pub fn ui() -> Result<Self, String> {
            Self::new(false)
        }
        fn new(worker: bool) -> Result<Self, String> {
            match unsafe {
                RoInitialize(if worker {
                    RO_INIT_MULTITHREADED
                } else {
                    RO_INIT_SINGLETHREADED
                })
            } {
                Ok(()) => Ok(Self(true)),
                Err(error) if error.code().0 == 0x80010106_u32 as i32 => Ok(Self(false)),
                Err(error) => Err(format!("Windows radio runtime is unavailable: {error}")),
            }
        }
    }
    impl Drop for Runtime {
        fn drop(&mut self) {
            if self.0 {
                unsafe { RoUninitialize() };
            }
        }
    }

    fn wait<T: RuntimeType + 'static>(
        operation: IAsyncOperation<T>,
        cancel: &AtomicBool,
        timeout: Duration,
    ) -> Result<T, String> {
        let start = Instant::now();
        loop {
            if cancel.load(Ordering::Relaxed) || start.elapsed() >= timeout {
                let _ = operation.Cancel();
                return Err("The radio operation was cancelled or timed out; refresh to see its current state.".into());
            }
            let status = operation.Status().map_err(|error| error.to_string())?;
            if status != AsyncStatus::Started {
                return operation
                    .GetResults()
                    .map_err(|error| format!("Windows radio operation failed: {error}"));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn enumerate(cancel: &AtomicBool) -> Result<Vec<Radio>, String> {
        let operation = Radio::GetRadiosAsync().map_err(|error| error.to_string())?;
        let radios = wait(operation, cancel, Duration::from_secs(10))?;
        let mut found = Vec::new();
        for index in 0..radios.Size().map_err(|error| error.to_string())? {
            let radio = radios.GetAt(index).map_err(|error| error.to_string())?;
            let kind = radio.Kind().map_err(|error| error.to_string())?;
            if kind == RadioKind::WiFi || kind == RadioKind::Bluetooth {
                found.push(radio);
            }
        }
        Ok(found)
    }

    fn describe(radio: &Radio) -> Result<WirelessRadio, String> {
        let kind = radio.Kind().map_err(|error| error.to_string())?;
        let state = radio.State().map_err(|error| error.to_string())?;
        Ok(WirelessRadio {
            name: radio.Name().map_err(|error| error.to_string())?.to_string(),
            kind: if kind == RadioKind::WiFi {
                WirelessKind::WiFi
            } else {
                WirelessKind::Bluetooth
            },
            state: match state {
                RadioState::On => WirelessState::On,
                RadioState::Off => WirelessState::Off,
                RadioState::Disabled => WirelessState::Disabled,
                _ => WirelessState::Unknown,
            },
        })
    }

    pub fn read(cancel: &AtomicBool) -> Result<Vec<WirelessRadio>, String> {
        let _runtime = Runtime::new(true)?;
        enumerate(cancel)?.iter().map(describe).collect()
    }

    pub fn request_access() -> Result<IAsyncOperation<RadioAccessStatus>, String> {
        Radio::RequestAccessAsync()
            .map_err(|error| format!("Radio access could not be requested: {error}"))
    }

    fn access_status(status: RadioAccessStatus) -> RadioAccess {
        match status {
            RadioAccessStatus::Allowed => RadioAccess::Allowed,
            RadioAccessStatus::DeniedByUser => RadioAccess::DeniedByUser,
            RadioAccessStatus::DeniedBySystem => RadioAccess::DeniedBySystem,
            _ => RadioAccess::Unspecified,
        }
    }

    fn authorize(
        permission: Option<IAsyncOperation<RadioAccessStatus>>,
        confirmed_access: &mut Option<RadioAccess>,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let access = if let Some(permission) = permission {
            access_status(wait(permission, cancel, Duration::from_secs(60))?)
        } else {
            confirmed_access.unwrap_or(RadioAccess::Unspecified)
        };
        *confirmed_access = Some(access);
        access.check()
    }

    fn set_and_confirm(
        radio: &Radio,
        enabled: bool,
        confirmed_access: &mut Option<RadioAccess>,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Radio change cancelled.".into());
        }
        let target = if enabled {
            RadioState::On
        } else {
            RadioState::Off
        };
        let access = access_status(wait(
            radio
                .SetStateAsync(target)
                .map_err(|error| error.to_string())?,
            cancel,
            Duration::from_secs(10),
        )?);
        *confirmed_access = Some(access);
        access.check()?;
        // Acceptance is not confirmation. Bound the driver's settling interval.
        for _ in 0..10 {
            if radio.State().map_err(|error| error.to_string())? == target {
                return Ok(());
            }
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err("Windows accepted the radio request, but the driver has not confirmed the requested state. Refresh to check.".into())
    }

    pub fn set(
        selected: WirelessRadio,
        enabled: bool,
        permission: Option<IAsyncOperation<RadioAccessStatus>>,
        cached: Option<RadioAccess>,
        cancel: &AtomicBool,
    ) -> RadioEvent {
        let mut confirmed_access = cached;
        let outcome = (|| {
            let _runtime = Runtime::new(true)?;
            authorize(permission, &mut confirmed_access, cancel)?;
            let mut found = Vec::new();
            for radio in enumerate(cancel)? {
                let info = describe(&radio)?;
                if info.kind == selected.kind && info.name == selected.name {
                    found.push(radio);
                }
            }
            if found.len() != 1 {
                return Err("The selected radio is missing or ambiguous; refresh the list.".into());
            }
            let radio = found.pop().unwrap();
            set_and_confirm(&radio, enabled, &mut confirmed_access, cancel)
        })();
        let status = Some(match outcome {
            Ok(()) => (
                format!(
                    "{} {}.",
                    selected.kind.label(),
                    if enabled { "enabled" } else { "disabled" }
                ),
                false,
            ),
            Err(error) => (error, true),
        });
        RadioEvent {
            radios: read(cancel),
            status,
            access: confirmed_access,
        }
    }

    pub fn toggle_wifi(
        permission: Option<IAsyncOperation<RadioAccessStatus>>,
        cached: Option<RadioAccess>,
        cancel: &AtomicBool,
    ) -> RadioEvent {
        let mut confirmed_access = cached;
        let mut target = None;
        let mut expected_names = Vec::new();
        let mut attempted_change = false;
        let outcome = (|| {
            let _runtime = Runtime::new(true)?;
            authorize(permission, &mut confirmed_access, cancel)?;
            let mut selected = Vec::new();
            for radio in enumerate(cancel)? {
                if radio.Kind().map_err(|error| error.to_string())? == RadioKind::WiFi {
                    selected.push(radio);
                }
            }
            let infos = selected
                .iter()
                .map(describe)
                .collect::<Result<Vec<_>, _>>()?;
            let enabled = wifi_toggle_target(&infos)?;
            target = Some(enabled);
            expected_names = infos.iter().map(|radio| radio.name.clone()).collect();
            for radio in selected {
                attempted_change = true;
                set_and_confirm(&radio, enabled, &mut confirmed_access, cancel)?;
            }
            Ok(())
        })();
        // A fresh read confirms the complete Wi-Fi group and updates the UI
        // even if an intermediate setter failed. No rollback is inferred.
        let radios = read(cancel);
        let outcome = outcome.and_then(|()| {
            let enabled = target.ok_or("No Wi-Fi change was requested.")?;
            let current = radios
                .as_ref()
                .map_err(|error| format!("Could not confirm the Wi-Fi request: {error}"))?;
            verify_wifi_toggle(current, &expected_names, enabled)
        });
        let status = match outcome {
            Ok(()) => (
                format!(
                    "Wi-Fi {}.",
                    if target == Some(true) {
                        "enabled"
                    } else {
                        "disabled"
                    }
                ),
                false,
            ),
            Err(error) if attempted_change => (
                format!("{error} Some Wi-Fi adapters may already have changed."),
                true,
            ),
            Err(error) => (error, true),
        };
        RadioEvent {
            radios,
            status: Some(status),
            access: confirmed_access,
        }
    }
}

#[cfg(windows)]
mod real {
    use windows::core::{w, GUID, PCWSTR};
    use windows::Win32::Foundation::{LocalFree, ERROR_SUCCESS, HLOCAL};
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{
        eMultimedia, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Power::{
        GetSystemPowerStatus, PowerGetActiveScheme, PowerReadACValueIndex, PowerReadDCValueIndex,
        PowerSetActiveScheme, PowerWriteACValueIndex, PowerWriteDCValueIndex, SYSTEM_POWER_STATUS,
    };
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    use super::{Brightness, SettingsPage, Volume};

    const VIDEO_SUBGROUP: GUID = GUID::from_u128(0x7516b95f_f776_4464_8c53_06167f40cc99);
    const VIDEO_BRIGHTNESS: GUID = GUID::from_u128(0xaded5e82_b909_4619_9949_f5d71dac0bcb);
    const RPC_E_CHANGED_MODE: i32 = 0x80010106_u32 as i32;

    struct ComGuard(bool);

    impl ComGuard {
        fn new() -> Result<Self, String> {
            let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            if result.is_ok() {
                Ok(Self(true))
            } else if result.0 == RPC_E_CHANGED_MODE {
                // Eframe or another component already owns this thread's COM apartment.
                Ok(Self(false))
            } else {
                Err(format!("Audio COM initialization failed: {result}"))
            }
        }
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    fn with_endpoint<T>(
        action: impl FnOnce(&IAudioEndpointVolume) -> Result<T, String>,
    ) -> Result<T, String> {
        let _com = ComGuard::new()?;
        let enumerator: IMMDeviceEnumerator = unsafe {
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("Audio endpoint enumeration failed: {e}"))?
        };
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) }
            .map_err(|e| format!("No default output device: {e}"))?;
        let endpoint: IAudioEndpointVolume = unsafe { device.Activate(CLSCTX_ALL, None) }
            .map_err(|e| format!("Output volume is unavailable: {e}"))?;
        action(&endpoint)
    }

    fn read_endpoint(endpoint: &IAudioEndpointVolume) -> Result<Volume, String> {
        let scalar = unsafe { endpoint.GetMasterVolumeLevelScalar() }
            .map_err(|e| format!("Could not read volume: {e}"))?;
        if !scalar.is_finite() || !(0.0..=1.0).contains(&scalar) {
            return Err("The output device returned an invalid volume.".into());
        }
        let muted = unsafe { endpoint.GetMute() }
            .map_err(|e| format!("Could not read mute state: {e}"))?
            .as_bool();
        Ok(Volume {
            percent: (scalar * 100.0).round() as u8,
            muted,
        })
    }

    pub fn volume() -> Result<Volume, String> {
        with_endpoint(read_endpoint)
    }

    pub fn set_volume(percent: u8) -> Result<Volume, String> {
        with_endpoint(|endpoint| {
            unsafe {
                endpoint.SetMasterVolumeLevelScalar(f32::from(percent) / 100.0, std::ptr::null())
            }
            .map_err(|e| format!("Could not set volume: {e}"))?;
            // SmartManager's volume control mutes at zero and unmutes above zero.
            unsafe { endpoint.SetMute(percent == 0, std::ptr::null()) }
                .map_err(|e| format!("Volume changed, but mute could not be updated: {e}"))?;
            let read = read_endpoint(endpoint)?;
            if read.percent == percent && read.muted == (percent == 0) {
                Ok(read)
            } else {
                Err(format!(
                    "The output reports {}% after setting {percent}%.",
                    read.percent
                ))
            }
        })
    }

    pub fn set_mute(muted: bool) -> Result<Volume, String> {
        with_endpoint(|endpoint| {
            unsafe { endpoint.SetMute(muted, std::ptr::null()) }
                .map_err(|e| format!("Could not change mute: {e}"))?;
            let read = read_endpoint(endpoint)?;
            if read.muted == muted {
                Ok(read)
            } else {
                Err("The output device did not confirm its mute state.".into())
            }
        })
    }

    struct ActiveScheme(*mut GUID);

    impl ActiveScheme {
        fn open() -> Result<Self, String> {
            let mut scheme = std::ptr::null_mut();
            let result = unsafe { PowerGetActiveScheme(None, &mut scheme) };
            if result != ERROR_SUCCESS || scheme.is_null() {
                return Err(format!(
                    "Could not read the active power plan ({})",
                    result.0
                ));
            }
            Ok(Self(scheme))
        }
    }

    impl Drop for ActiveScheme {
        fn drop(&mut self) {
            unsafe { LocalFree(Some(HLOCAL(self.0.cast()))) };
        }
    }

    fn on_ac() -> Result<bool, String> {
        let mut status = SYSTEM_POWER_STATUS::default();
        unsafe { GetSystemPowerStatus(&mut status) }
            .map_err(|e| format!("Could not read the power source: {e}"))?;
        match status.ACLineStatus {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("Windows does not currently report the AC/battery source.".into()),
        }
    }

    fn read_brightness(scheme: &ActiveScheme, ac: bool) -> Result<Brightness, String> {
        let mut percent = 0;
        let result = unsafe {
            if ac {
                PowerReadACValueIndex(
                    None,
                    Some(scheme.0),
                    Some(&VIDEO_SUBGROUP),
                    Some(&VIDEO_BRIGHTNESS),
                    &mut percent,
                )
                .0
            } else {
                PowerReadDCValueIndex(
                    None,
                    Some(scheme.0),
                    Some(&VIDEO_SUBGROUP),
                    Some(&VIDEO_BRIGHTNESS),
                    &mut percent,
                )
            }
        };
        if result != ERROR_SUCCESS.0 {
            return Err(format!(
                "Brightness is unavailable for this power plan ({result})"
            ));
        }
        if percent > 100 {
            return Err(format!(
                "The power plan returned invalid brightness {percent}%."
            ));
        }
        Ok(Brightness {
            percent: percent as u8,
            on_ac: ac,
        })
    }

    pub fn brightness() -> Result<Brightness, String> {
        read_brightness(&ActiveScheme::open()?, on_ac()?)
    }

    pub fn set_brightness(percent: u8) -> Result<Brightness, String> {
        let scheme = ActiveScheme::open()?;
        let ac = on_ac()?;
        let result = unsafe {
            if ac {
                PowerWriteACValueIndex(
                    None,
                    scheme.0,
                    Some(&VIDEO_SUBGROUP),
                    Some(&VIDEO_BRIGHTNESS),
                    u32::from(percent),
                )
                .0
            } else {
                PowerWriteDCValueIndex(
                    None,
                    scheme.0,
                    Some(&VIDEO_SUBGROUP),
                    Some(&VIDEO_BRIGHTNESS),
                    u32::from(percent),
                )
            }
        };
        if result != ERROR_SUCCESS.0 {
            return Err(format!("Could not save the brightness setting ({result})"));
        }
        let result = unsafe { PowerSetActiveScheme(None, Some(scheme.0)) };
        if result != ERROR_SUCCESS {
            return Err(format!(
                "Brightness was saved but the power plan could not be reactivated ({})",
                result.0
            ));
        }
        let read = read_brightness(&scheme, ac)?;
        if read.percent == percent {
            Ok(read)
        } else {
            Err(format!(
                "The power plan reports {}% after setting {percent}%.",
                read.percent
            ))
        }
    }

    pub fn open_settings(page: SettingsPage) -> Result<(), String> {
        let target: Vec<u16> = page.uri().encode_utf16().chain(Some(0)).collect();
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(target.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if result.0 as isize > 32 {
            Ok(())
        } else {
            Err(format!(
                "Windows could not open settings (code {}).",
                result.0 as isize
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_controls_are_independent_and_read_back_exactly() {
        let mut controls = SystemControls::new(true);
        assert_eq!(controls.set_volume(37).unwrap().percent, 37);
        assert!(controls.set_mute(true).unwrap().muted);
        let read = controls.set_brightness(82).unwrap();
        assert_eq!(read.percent, 82);
        assert!(read.on_ac);
        assert_eq!(
            controls.volume().unwrap(),
            Volume {
                percent: 37,
                muted: true
            }
        );
        for page in SettingsPage::ALL {
            controls.open_settings(page).unwrap();
        }
    }

    #[test]
    fn out_of_range_inputs_do_not_change_the_mock() {
        let mut controls = SystemControls::new(true);
        let volume = controls.volume().unwrap();
        let brightness = controls.brightness().unwrap();
        assert!(controls.set_volume(101).is_err());
        assert!(controls.set_brightness(255).is_err());
        assert_eq!(controls.volume().unwrap(), volume);
        assert_eq!(controls.brightness().unwrap(), brightness);
    }

    #[test]
    fn volume_slider_follows_smartmanager_zero_mute_behavior() {
        let mut controls = SystemControls::new(true);
        assert!(controls.set_volume(0).unwrap().muted);
        let volume = controls.set_volume(35).unwrap();
        assert_eq!(volume.percent, 35);
        assert!(!volume.muted);
    }

    #[test]
    fn mock_radio_changes_only_the_selected_device() {
        let mut controls = WirelessControls::new(true);
        let before = controls.radios.clone().unwrap();
        controls.set(before[0].clone(), false).unwrap();
        let radios = controls.radios.as_ref().unwrap();
        assert_eq!(radios[0].state, WirelessState::Off);
        assert_eq!(radios[1], before[1]);
        assert!(!controls.busy());
        controls.refresh();
        assert_eq!(
            controls.radios.as_ref().unwrap()[0].state,
            WirelessState::Off
        );
    }

    #[test]
    fn denied_or_hardware_disabled_radio_does_not_change() {
        let mut controls = WirelessControls::new(true);
        let radio = controls.radios.as_ref().unwrap()[0].clone();
        controls.access = Some(RadioAccess::DeniedBySystem);
        assert!(controls.set(radio.clone(), false).is_err());
        controls.retry_access();
        let mut disabled = radio;
        disabled.state = WirelessState::Disabled;
        assert!(controls.set(disabled, false).is_err());
        assert_eq!(
            controls.radios.as_ref().unwrap()[0].state,
            WirelessState::On
        );
    }

    #[test]
    fn wifi_toggle_changes_the_wifi_group_without_touching_bluetooth() {
        let mut controls = WirelessControls::new(true);
        controls.radios.as_mut().unwrap().push(WirelessRadio {
            name: "Second Wi-Fi adapter".into(),
            kind: WirelessKind::WiFi,
            state: WirelessState::Off,
        });
        let bluetooth = controls.radios.as_ref().unwrap()[1].clone();
        controls.toggle_wifi().unwrap();
        let radios = controls.radios.as_ref().unwrap();
        assert_eq!(radios[0].state, WirelessState::Off);
        assert_eq!(radios[2].state, WirelessState::Off);
        assert_eq!(radios[1], bluetooth);
        controls.toggle_wifi().unwrap();
        let radios = controls.radios.as_ref().unwrap();
        assert_eq!(radios[0].state, WirelessState::On);
        assert_eq!(radios[2].state, WirelessState::On);
        assert_eq!(radios[1], bluetooth);
    }

    #[test]
    fn wifi_toggle_rejects_missing_unknown_disabled_or_denied_state_without_changes() {
        for state in [WirelessState::Unknown, WirelessState::Disabled] {
            let mut controls = WirelessControls::new(true);
            controls.radios.as_mut().unwrap()[0].state = state;
            let before = controls.radios.clone().unwrap();
            assert!(controls.toggle_wifi().is_err());
            assert_eq!(controls.radios.as_ref().unwrap(), &before);
        }
        let mut controls = WirelessControls::new(true);
        controls.access = Some(RadioAccess::DeniedByUser);
        let before = controls.radios.clone().unwrap();
        assert!(controls.toggle_wifi().is_err());
        assert_eq!(controls.radios.as_ref().unwrap(), &before);
        controls.retry_access();
        controls
            .radios
            .as_mut()
            .unwrap()
            .retain(|radio| radio.kind != WirelessKind::WiFi);
        assert!(controls.toggle_wifi().is_err());
    }

    #[test]
    fn partial_or_changed_wifi_readback_is_not_success() {
        let mut radios = WirelessControls::new(true).radios.clone().unwrap();
        let expected = vec![radios[0].name.clone(), "Second Wi-Fi adapter".into()];
        assert!(verify_wifi_toggle(&radios, &expected, true).is_err());
        radios.push(WirelessRadio {
            name: expected[1].clone(),
            kind: WirelessKind::WiFi,
            state: WirelessState::Off,
        });
        assert!(verify_wifi_toggle(&radios, &expected, true).is_err());
        radios[2].state = WirelessState::On;
        assert!(verify_wifi_toggle(&radios, &expected, true).is_ok());
        // A Bluetooth state change does not affect Wi-Fi confirmation.
        radios[1].state = WirelessState::Off;
        assert!(verify_wifi_toggle(&radios, &expected, true).is_ok());
    }

    #[test]
    fn wifi_requests_reject_overlap_and_poll_completed_work_without_refresh() {
        let mut controls = WirelessControls::new(true);
        let (tx, rx) = std::sync::mpsc::channel();
        controls.receiver = Some(rx);
        assert!(controls.toggle_wifi().is_err());
        let mut current = controls.radios.clone().unwrap();
        current[0].state = WirelessState::Off;
        tx.send(RadioEvent {
            radios: Ok(current.clone()),
            status: Some(("Wi-Fi disabled.".into(), false)),
            access: Some(RadioAccess::Allowed),
        })
        .unwrap();
        assert_eq!(controls.poll(), Some(("Wi-Fi disabled.".into(), false)));
        assert!(!controls.busy());
        assert_eq!(controls.radios.as_ref().unwrap(), &current);
        assert_eq!(controls.poll(), None);
        assert!(!controls.busy());
    }
}
