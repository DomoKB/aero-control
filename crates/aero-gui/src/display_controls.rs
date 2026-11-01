//! Display refresh rates. Enumerated modes only; no background polling.

use std::collections::VecDeque;
use std::sync::mpsc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplaySettings {
    pub automatic: bool,
    /// Monitor interface identity, never a mutable DISPLAY1/2 adapter number.
    pub display_id: Option<String>,
    pub ac_hz: Option<u32>,
    pub dc_hz: Option<u32>,
}

impl DisplaySettings {
    pub fn validate(&self) -> Result<(), String> {
        if self
            .display_id
            .as_ref()
            .is_some_and(|id| id.trim().is_empty())
        {
            return Err("The saved display identity is empty.".into());
        }
        if [self.ac_hz, self.dc_hz]
            .into_iter()
            .flatten()
            .any(|hz| hz <= 1)
        {
            return Err("Display refresh rates must be greater than 1 Hz.".into());
        }
        if self.automatic
            && (self.display_id.is_none() || self.ac_hz.is_none() || self.dc_hz.is_none())
        {
            return Err(
                "Choose a display and both power-source refresh rates before enabling switching."
                    .into(),
            );
        }
        Ok(())
    }

    pub fn from_table(root: &toml::Table) -> Result<Self, String> {
        let Some(value) = root.get("display") else {
            return Ok(Self::default());
        };
        let table = value.as_table().ok_or("display must be a table")?;
        let automatic = match table.get("automatic") {
            None => false,
            Some(value) => value
                .as_bool()
                .ok_or("display.automatic must be true or false")?,
        };
        let display_id = table
            .get("display_id")
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or("display.display_id must be text")
            })
            .transpose()?;
        let frequency = |key| -> Result<Option<u32>, String> {
            table
                .get(key)
                .map(|value| {
                    value
                        .as_integer()
                        .and_then(|n| u32::try_from(n).ok())
                        .filter(|hz| *hz > 1)
                        .ok_or_else(|| format!("display.{key} must be an integer greater than 1"))
                })
                .transpose()
        };
        let settings = Self {
            automatic,
            display_id,
            ac_hz: frequency("ac_hz")?,
            dc_hz: frequency("dc_hz")?,
        };
        settings.validate()?;
        Ok(settings)
    }

    pub fn write_table(&self, root: &mut toml::Table) {
        let mut table = toml::Table::new();
        table.insert("automatic".into(), self.automatic.into());
        if let Some(id) = &self.display_id {
            table.insert("display_id".into(), id.clone().into());
        }
        if let Some(hz) = self.ac_hz {
            table.insert("ac_hz".into(), i64::from(hz).into());
        }
        if let Some(hz) = self.dc_hz {
            table.insert("dc_hz".into(), i64::from(hz).into());
        }
        root.insert("display".into(), table.into());
    }

    pub fn target(&self, on_ac: Option<bool>) -> Option<(&str, u32)> {
        if !self.automatic {
            return None;
        }
        let hz = if on_ac? { self.ac_hz? } else { self.dc_hz? };
        Some((self.display_id.as_deref()?, hz))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub bits_per_pixel: u32,
    pub orientation: u32,
    pub flags: u32,
    pub hz: u32,
}

impl DisplayMode {
    fn same_format(self, other: Self) -> bool {
        self.width == other.width
            && self.height == other.height
            && self.bits_per_pixel == other.bits_per_pixel
            && self.orientation == other.orientation
            && self.flags == other.flags
    }
}

pub fn frequencies(current: DisplayMode, modes: impl IntoIterator<Item = DisplayMode>) -> Vec<u32> {
    let mut rates: Vec<_> = modes
        .into_iter()
        .filter(|mode| mode.same_format(current) && mode.hz > 1)
        .map(|mode| mode.hz)
        .collect();
    rates.sort_unstable();
    rates.dedup();
    rates
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayInfo {
    pub id: String,
    pub device_name: String,
    pub label: String,
    pub stable_identity: bool,
    pub current: DisplayMode,
    pub frequencies: Vec<u32>,
}

fn selected_display<'a>(
    displays: &'a [DisplayInfo],
    id: &str,
    hz: u32,
) -> Result<&'a DisplayInfo, String> {
    let mut matches = displays.iter().filter(|display| display.id == id);
    let display = matches
        .next()
        .ok_or("The selected display is no longer attached. Refresh displays.")?;
    if matches.next().is_some() {
        return Err("The selected display identity is ambiguous; no display was changed.".into());
    }
    if !display.frequencies.contains(&hz) {
        return Err(format!("{hz} Hz is not an enumerated mode at this display's current resolution, colour depth and orientation."));
    }
    Ok(display)
}

enum Command {
    Read,
    Apply {
        id: String,
        hz: u32,
        automatic: bool,
    },
}
struct Event {
    displays: Result<Vec<DisplayInfo>, String>,
    status: Option<(String, bool)>,
}

pub struct DisplayControls {
    pub displays: Result<Vec<DisplayInfo>, String>,
    mock: Option<Vec<DisplayInfo>>,
    sender: Option<mpsc::Sender<Command>>,
    receiver: mpsc::Receiver<Event>,
    pending: usize,
    statuses: VecDeque<(String, bool)>,
}

impl DisplayControls {
    pub fn new(mock: bool) -> Self {
        let mock = mock || !cfg!(windows);
        let (events, receiver) = mpsc::channel();
        let mut controls = Self {
            displays: Err("Display modes have not been read yet.".into()),
            mock: None,
            sender: None,
            receiver,
            pending: 0,
            statuses: VecDeque::new(),
        };
        if mock {
            controls.mock = Some(vec![DisplayInfo {
                id: "mock-internal-display".into(),
                device_name: "Mock display".into(),
                label: "Mock built-in display".into(),
                stable_identity: true,
                current: DisplayMode {
                    width: 1920,
                    height: 1080,
                    bits_per_pixel: 32,
                    orientation: 0,
                    flags: 0,
                    hz: 60,
                },
                frequencies: vec![60, 144],
            }]);
            return controls;
        }
        #[cfg(windows)]
        {
            let (sender, commands) = mpsc::channel();
            match std::thread::Builder::new()
                .name("aero-display-controls".into())
                .spawn(move || {
                    while let Ok(command) = commands.recv() {
                        let event = match command {
                            Command::Read => Event {
                                displays: native::read(),
                                status: None,
                            },
                            Command::Apply { id, hz, automatic } => {
                                let status = match native::apply(&id, hz, automatic) {
                                    Ok(()) => (
                                        format!(
                                            "{}Display refresh rate confirmed at {hz} Hz.",
                                            if automatic {
                                                "Power-source switch: "
                                            } else {
                                                ""
                                            }
                                        ),
                                        false,
                                    ),
                                    Err(error) => (error, true),
                                };
                                Event {
                                    displays: native::read(),
                                    status: Some(status),
                                }
                            }
                        };
                        if events.send(event).is_err() {
                            break;
                        }
                    }
                }) {
                Ok(_) => controls.sender = Some(sender),
                Err(error) => {
                    controls.displays = Err(format!("Could not start display controls: {error}"))
                }
            }
        }
        #[cfg(not(windows))]
        {
            drop(events);
        }
        controls
    }

    pub fn busy(&self) -> bool {
        self.pending != 0
    }
    pub fn is_mock(&self) -> bool {
        self.mock.is_some()
    }

    pub fn refresh(&mut self) -> Result<(), String> {
        if let Some(displays) = &self.mock {
            self.displays = Ok(displays.clone());
            return Ok(());
        }
        self.send(Command::Read)
    }

    fn send(&mut self, command: Command) -> Result<(), String> {
        self.sender
            .as_ref()
            .ok_or("The display worker is unavailable.")?
            .send(command)
            .map_err(|_| "The display worker stopped unexpectedly.")?;
        self.pending += 1;
        Ok(())
    }

    pub fn apply(&mut self, id: &str, hz: u32, automatic: bool) -> Result<(), String> {
        if let Some(displays) = &mut self.mock {
            let display = selected_display(displays, id, hz)?;
            if automatic && !display.stable_identity {
                return Err("Automatic switching requires a stable monitor identity.".into());
            }
            displays
                .iter_mut()
                .find(|display| display.id == id)
                .unwrap()
                .current
                .hz = hz;
            self.displays = Ok(displays.clone());
            self.statuses.push_back((
                format!("Mock: display refresh rate confirmed at {hz} Hz."),
                false,
            ));
            return Ok(());
        }
        self.send(Command::Apply {
            id: id.into(),
            hz,
            automatic,
        })
    }

    pub fn poll(&mut self) -> Option<(String, bool)> {
        loop {
            match self.receiver.try_recv() {
                Ok(event) => {
                    self.pending = self.pending.saturating_sub(1);
                    self.displays = event.displays;
                    if let Some(status) = event.status {
                        self.statuses.push_back(status);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if self.pending != 0 {
                        self.pending = 0;
                        self.displays = Err("The display worker stopped unexpectedly.".into());
                        self.statuses
                            .push_back(("The display worker stopped unexpectedly.".into(), true));
                    }
                    break;
                }
            }
        }
        self.statuses.pop_front()
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::{
        ChangeDisplaySettingsExW, EnumDisplayDevicesW, EnumDisplaySettingsW, CDS_TEST, CDS_TYPE,
        DEVMODEW, DISPLAY_DEVICEW, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP,
        DISPLAY_DEVICE_MIRRORING_DRIVER, DISP_CHANGE_SUCCESSFUL, DM_DISPLAYFREQUENCY,
        ENUM_CURRENT_SETTINGS, ENUM_DISPLAY_SETTINGS_MODE,
    };
    use windows::Win32::UI::WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME;

    fn text(value: &[u16]) -> String {
        String::from_utf16_lossy(
            &value[..value.iter().position(|c| *c == 0).unwrap_or(value.len())],
        )
    }
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
    fn empty_mode() -> DEVMODEW {
        DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..Default::default()
        }
    }
    fn mode_info(mode: &DEVMODEW) -> DisplayMode {
        DisplayMode {
            width: mode.dmPelsWidth,
            height: mode.dmPelsHeight,
            bits_per_pixel: mode.dmBitsPerPel,
            orientation: unsafe { mode.Anonymous1.Anonymous2.dmDisplayOrientation.0 },
            flags: unsafe { mode.Anonymous2.dmDisplayFlags },
            hz: mode.dmDisplayFrequency,
        }
    }
    fn current(name: &[u16]) -> Result<DEVMODEW, String> {
        let mut mode = empty_mode();
        if unsafe { EnumDisplaySettingsW(PCWSTR(name.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode) }
            .as_bool()
        {
            Ok(mode)
        } else {
            Err("Windows could not read the display's current mode.".into())
        }
    }

    pub fn read() -> Result<Vec<DisplayInfo>, String> {
        let mut displays = Vec::new();
        for index in 0..128 {
            let mut adapter = DISPLAY_DEVICEW {
                cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
                ..Default::default()
            };
            if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), index, &mut adapter, 0) }.as_bool() {
                break;
            }
            if !adapter
                .StateFlags
                .contains(DISPLAY_DEVICE_ATTACHED_TO_DESKTOP)
                || adapter.StateFlags.contains(DISPLAY_DEVICE_MIRRORING_DRIVER)
            {
                continue;
            }
            let device_name = text(&adapter.DeviceName);
            let name = wide(&device_name);
            let current = mode_info(&current(&name)?);
            let mut identities = Vec::new();
            let mut labels = Vec::new();
            for monitor_index in 0..128 {
                let mut monitor = DISPLAY_DEVICEW {
                    cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
                    ..Default::default()
                };
                if !unsafe {
                    EnumDisplayDevicesW(
                        PCWSTR(name.as_ptr()),
                        monitor_index,
                        &mut monitor,
                        EDD_GET_DEVICE_INTERFACE_NAME,
                    )
                }
                .as_bool()
                {
                    break;
                }
                if !monitor
                    .StateFlags
                    .contains(DISPLAY_DEVICE_ATTACHED_TO_DESKTOP)
                {
                    continue;
                }
                let identity = text(&monitor.DeviceID);
                if !identity.is_empty() {
                    identities.push(identity);
                }
                labels.push(text(&monitor.DeviceString));
            }
            identities.sort();
            identities.dedup();
            let stable_identity = !identities.is_empty();
            let id = if stable_identity {
                identities.join("|")
            } else {
                format!("adapter:{device_name}")
            };
            let label = if labels.is_empty() {
                text(&adapter.DeviceString)
            } else {
                labels.join(" + ")
            };
            let mut modes = Vec::new();
            for mode_index in 0..4096 {
                let mut mode = empty_mode();
                if !unsafe {
                    EnumDisplaySettingsW(
                        PCWSTR(name.as_ptr()),
                        ENUM_DISPLAY_SETTINGS_MODE(mode_index),
                        &mut mode,
                    )
                }
                .as_bool()
                {
                    break;
                }
                modes.push(mode_info(&mode));
            }
            displays.push(DisplayInfo {
                id,
                device_name,
                label,
                stable_identity,
                current,
                frequencies: frequencies(current, modes),
            });
        }
        Ok(displays)
    }

    fn restore(name: &[u16], previous: &DEVMODEW) -> Result<(), String> {
        let code = unsafe {
            ChangeDisplaySettingsExW(
                PCWSTR(name.as_ptr()),
                Some(previous),
                None,
                CDS_TYPE(0),
                None,
            )
        };
        if code != DISP_CHANGE_SUCCESSFUL {
            return Err(format!(
                "restoring the previous display mode failed ({})",
                code.0
            ));
        }
        if mode_info(&current(name)?) != mode_info(previous) {
            return Err("the previous display mode could not be confirmed".into());
        }
        Ok(())
    }

    pub fn apply(id: &str, hz: u32, automatic: bool) -> Result<(), String> {
        let displays = read()?;
        let display = selected_display(&displays, id, hz)?;
        if automatic && !display.stable_identity {
            return Err("Automatic switching requires a stable monitor identity.".into());
        }
        let name = wide(&display.device_name);
        let previous = current(&name)?;
        if mode_info(&previous) != display.current {
            return Err(
                "The display mode changed during the request. Refresh and try again.".into(),
            );
        }
        let mut requested = previous;
        requested.dmDriverExtra = 0;
        requested.dmDisplayFrequency = hz;
        requested.dmFields |= DM_DISPLAYFREQUENCY;
        let code = unsafe {
            ChangeDisplaySettingsExW(
                PCWSTR(name.as_ptr()),
                Some(&requested),
                None,
                CDS_TEST,
                None,
            )
        };
        if code != DISP_CHANGE_SUCCESSFUL {
            return Err(format!(
                "Windows rejected the {hz} Hz display-mode test ({}); no change was applied.",
                code.0
            ));
        }
        let code = unsafe {
            ChangeDisplaySettingsExW(
                PCWSTR(name.as_ptr()),
                Some(&requested),
                None,
                CDS_TYPE(0),
                None,
            )
        };
        let result = if code != DISP_CHANGE_SUCCESSFUL {
            Err(format!(
                "Windows did not apply the display refresh rate ({}).",
                code.0
            ))
        } else {
            current(&name).and_then(|mode| {
                if mode_info(&mode) == mode_info(&requested) {
                    Ok(())
                } else {
                    Err("Windows accepted the request, but display readback did not match.".into())
                }
            })
        };
        if let Err(error) = result {
            return Err(match restore(&name, &previous) {
                Ok(()) => format!("{error} The previous mode was restored and confirmed."),
                Err(restore) => format!("{error} Recovery failed: {restore}."),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rates_are_unique_and_keep_resolution_colour_orientation_and_flags() {
        let current = DisplayMode {
            width: 1920,
            height: 1080,
            bits_per_pixel: 32,
            orientation: 0,
            flags: 0,
            hz: 60,
        };
        assert_eq!(
            frequencies(
                current,
                [
                    current,
                    DisplayMode { hz: 144, ..current },
                    current,
                    DisplayMode {
                        width: 1280,
                        hz: 240,
                        ..current
                    },
                    DisplayMode {
                        bits_per_pixel: 16,
                        hz: 75,
                        ..current
                    },
                    DisplayMode {
                        orientation: 1,
                        hz: 90,
                        ..current
                    },
                    DisplayMode {
                        flags: 2,
                        hz: 120,
                        ..current
                    },
                    DisplayMode { hz: 1, ..current }
                ]
            ),
            vec![60, 144]
        );
    }
    #[test]
    fn mock_rejects_unlisted_rate_without_changing_the_display() {
        let mut controls = DisplayControls::new(true);
        controls.refresh().unwrap();
        let before = controls.displays.clone().unwrap();
        assert!(controls.apply(&before[0].id, 240, false).is_err());
        assert_eq!(controls.displays.as_ref().unwrap(), &before);
        controls.apply(&before[0].id, 144, false).unwrap();
        assert_eq!(controls.displays.as_ref().unwrap()[0].current.hz, 144);
        assert!(!controls.busy());
    }
    #[test]
    fn settings_are_opt_in_and_strictly_typed() {
        let mut root = toml::Table::new();
        assert_eq!(
            DisplaySettings::from_table(&root).unwrap(),
            DisplaySettings::default()
        );
        let settings = DisplaySettings {
            automatic: true,
            display_id: Some("monitor-id".into()),
            ac_hz: Some(144),
            dc_hz: Some(60),
        };
        settings.write_table(&mut root);
        assert_eq!(DisplaySettings::from_table(&root).unwrap(), settings);
        assert_eq!(settings.target(Some(false)), Some(("monitor-id", 60)));
        assert_eq!(settings.target(None), None);
        root["display"]
            .as_table_mut()
            .unwrap()
            .insert("automatic".into(), "true".into());
        assert!(DisplaySettings::from_table(&root).is_err());
    }
}
