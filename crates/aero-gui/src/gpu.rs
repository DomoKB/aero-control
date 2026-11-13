//! NVIDIA availability from Windows' device tree, without opening a GPU driver.
//!
//! A headless Optimus adapter is still usable: an attached display is not
//! required. Missing, disabled and failed devices do not receive GPU commands.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GpuStatus {
    Available,
    Unavailable,
    Unknown(String),
}

impl GpuStatus {
    pub fn allows_commands(&self) -> bool {
        matches!(self, Self::Available)
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Available => "NVIDIA GPU available",
            Self::Unavailable => "NVIDIA GPU off or unavailable",
            Self::Unknown(_) => "NVIDIA GPU status unavailable",
        }
    }

    pub fn explanation(&self) -> String {
        match self {
            Self::Available => "Fan modes also apply their usual NVIDIA settings.".into(),
            Self::Unavailable => {
                "NVIDIA GPU off or unavailable. Fan control works without NVIDIA power requests or GPU temperature readings.".into()
            }
            Self::Unknown(error) => format!(
                "Windows could not determine NVIDIA availability: {error}. NVIDIA power requests and GPU temperature readings are skipped."
            ),
        }
    }
}

pub fn detect(mock: bool) -> GpuStatus {
    if mock {
        return GpuStatus::Available;
    }
    #[cfg(windows)]
    {
        native::detect()
    }
    #[cfg(not(windows))]
    {
        GpuStatus::Unknown("GPU detection is only supported on Windows".into())
    }
}

#[cfg(any(windows, test))]
fn is_nvidia_pci(instance_id: &str) -> bool {
    let upper = instance_id.to_ascii_uppercase();
    let Some(device) = upper.strip_prefix("PCI\\") else {
        return false;
    };
    device
        .split('\\')
        .next()
        .unwrap_or_default()
        .split('&')
        .any(|part| part == "VEN_10DE")
}

#[cfg(any(windows, test))]
fn classify(states: impl IntoIterator<Item = Result<bool, String>>) -> GpuStatus {
    let mut available = false;
    for state in states {
        match state {
            Ok(started_without_problem) => available |= started_without_problem,
            Err(error) => return GpuStatus::Unknown(error),
        }
    }
    if available {
        GpuStatus::Available
    } else {
        GpuStatus::Unavailable
    }
}

#[cfg(windows)]
mod native {
    use super::{classify, is_nvidia_pci, GpuStatus};
    use windows::core::{w, HRESULT};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_DevNode_Status, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo,
        SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW, CM_DEVNODE_STATUS_FLAGS, CM_PROB,
        CR_SUCCESS, DIGCF_PRESENT, DN_HAS_PROBLEM, DN_STARTED, GUID_DEVCLASS_DISPLAY, HDEVINFO,
        SP_DEVINFO_DATA,
    };
    use windows::Win32::Foundation::ERROR_NO_MORE_ITEMS;

    struct DeviceList(HDEVINFO);

    impl Drop for DeviceList {
        fn drop(&mut self) {
            // SAFETY: this owns the successfully created SetupAPI information set.
            unsafe {
                let _ = SetupDiDestroyDeviceInfoList(self.0);
            }
        }
    }

    pub fn detect() -> GpuStatus {
        match nvidia_states() {
            Ok(states) => classify(states.into_iter().map(Ok)),
            Err(error) => GpuStatus::Unknown(error),
        }
    }

    fn nvidia_states() -> Result<Vec<bool>, String> {
        // SAFETY: the class GUID and constant enumerator are valid; no parent
        // window is needed for read-only enumeration. The set is released by RAII.
        let set = DeviceList(
            unsafe {
                SetupDiGetClassDevsW(Some(&GUID_DEVCLASS_DISPLAY), w!("PCI"), None, DIGCF_PRESENT)
            }
            .map_err(|error| format!("enumerating display adapters failed ({error})"))?,
        );
        let mut states = Vec::new();
        for index in 0.. {
            let mut device = SP_DEVINFO_DATA {
                cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            // SAFETY: device has its required size and the information set is live.
            if let Err(error) = unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut device) } {
                if error.code() == HRESULT::from_win32(ERROR_NO_MORE_ITEMS.0) {
                    return Ok(states);
                }
                return Err(format!("reading a display adapter failed ({error})"));
            }
            // Windows device instance IDs are at most MAX_DEVICE_ID_LEN (200)
            // characters. Any API failure, including an oversized ID, fails closed.
            let mut id = [0u16; 512];
            // SAFETY: the returned device belongs to this set; the API receives
            // the buffer's full length through the slice wrapper.
            unsafe { SetupDiGetDeviceInstanceIdW(set.0, &device, Some(&mut id), None) }
                .map_err(|error| format!("reading a display adapter identity failed ({error})"))?;
            let end = id
                .iter()
                .position(|value| *value == 0)
                .ok_or_else(|| "display adapter identity was not terminated".to_string())?;
            let id = String::from_utf16(&id[..end])
                .map_err(|_| "display adapter identity was not valid UTF-16".to_string())?;
            if !is_nvidia_pci(&id) {
                continue;
            }
            let mut status = CM_DEVNODE_STATUS_FLAGS::default();
            let mut problem = CM_PROB::default();
            // SAFETY: DevInst came from the local device information set. Flags
            // must be zero; both output pointers refer to initialized storage.
            let result =
                unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, device.DevInst, 0) };
            if result != CR_SUCCESS {
                return Err(format!(
                    "reading NVIDIA device status failed ({})",
                    result.0
                ));
            }
            states.push(status.0 & DN_STARTED.0 != 0 && status.0 & DN_HAS_PROBLEM.0 == 0);
        }
        unreachable!("the device index range is unbounded")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pci_vendor_matching_does_not_match_other_adapters_or_id_substrings() {
        assert!(is_nvidia_pci(r"PCI\VEN_10DE&DEV_1ED3&SUBSYS_86B01458\1"));
        assert!(is_nvidia_pci(r"pci\ven_10de&dev_1ed3\1"));
        for id in [
            r"PCI\VEN_8086&DEV_9BC4\1",
            r"ROOT\DISPLAY\VEN_10DE",
            r"USB\VEN_10DE&PID_1234\1",
            r"PCI\VEN_10DE0&DEV_1234\1",
            r"PCI\VEN_8086\VEN_10DE",
        ] {
            assert!(!is_nvidia_pci(id), "{id}");
        }
    }

    #[test]
    fn absent_and_stopped_adapters_do_not_enable_gpu_commands() {
        assert_eq!(classify([]), GpuStatus::Unavailable);
        assert_eq!(classify([Ok(false)]), GpuStatus::Unavailable);
        assert!(!classify([Ok(false), Ok(false)]).allows_commands());
    }

    #[test]
    fn working_adapter_does_not_need_an_attached_display() {
        assert_eq!(classify([Ok(true)]), GpuStatus::Available);
        assert_eq!(classify([Ok(false), Ok(true)]), GpuStatus::Available);
    }

    #[test]
    fn detection_errors_are_not_hidden_by_an_available_adapter() {
        assert_eq!(
            classify([Ok(true), Err("status failed".into())]),
            GpuStatus::Unknown("status failed".into())
        );
        assert!(!GpuStatus::Unknown("status failed".into()).allows_commands());
    }

    #[test]
    fn mock_detection_never_uses_the_host_device_tree() {
        assert_eq!(detect(true), GpuStatus::Available);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Reads the host Windows PnP device tree; never changes hardware"]
    fn native_gpu_detection_is_read_only() {
        let status = detect(false);
        eprintln!("{}: {}", status.label(), status.explanation());
        assert!(!matches!(status, GpuStatus::Unknown(_)), "{status:?}");
    }
}
