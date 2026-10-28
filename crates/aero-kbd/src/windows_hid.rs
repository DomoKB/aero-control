//! Windows HID backend: SetupAPI enumeration + HidD feature reports, the same
//! calls Control Center makes. Only a collection whose VID/PID is a known
//! built-in Gigabyte keyboard and whose feature report length is 9 is used.

use std::thread::sleep;
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetFeature, HidD_GetHidGuid,
    HidD_GetPreparsedData, HidD_SetFeature, HidP_GetCaps, HIDD_ATTRIBUTES, HIDP_CAPS,
    HIDP_STATUS_SUCCESS, PHIDP_PREPARSED_DATA,
};
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_OVERLAPPED,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::System::IO::{CancelIo, GetOverlappedResult, OVERLAPPED};

use crate::proto::{self, LightSettings, Report, MATRIX_CHUNK, MATRIX_LEN};
use crate::{
    verify, DeviceInfo, KbdError, KeyboardBackend, Verified, FEATURE_LEN, ITE_PRODUCT_IDS,
    VENDOR_IDS,
};

/// Control Center sleeps 65 ms after each 0x08 / 0x12 command.
const SETTLE: Duration = Duration::from_millis(65);
const IO_TIMEOUT_MS: u32 = 1000;

struct Handle(HANDLE);

// SAFETY: a Win32 file/event handle may be used from any thread; HidKeyboard is
// only ever used through &mut, so there is no concurrent access.
unsafe impl Send for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub struct HidKeyboard {
    handle: Handle,
    info: DeviceInfo,
    caps: HIDP_CAPS,
}

fn last_error(what: &str) -> KbdError {
    KbdError::Io(format!("{what}: {}", windows::core::Error::from_win32()))
}

fn open_path(path: &[u16], access: u32) -> Option<Handle> {
    unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(FILE_FLAG_OVERLAPPED.0),
            None,
        )
        .ok()
        .map(Handle)
    }
}

fn attributes_and_caps(h: &Handle) -> Option<(HIDD_ATTRIBUTES, HIDP_CAPS)> {
    unsafe {
        let mut attrs = HIDD_ATTRIBUTES {
            Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32,
            ..Default::default()
        };
        if !HidD_GetAttributes(h.0, &mut attrs) {
            return None;
        }
        let mut pre = PHIDP_PREPARSED_DATA::default();
        if !HidD_GetPreparsedData(h.0, &mut pre) {
            return None;
        }
        let mut caps = HIDP_CAPS::default();
        let status = HidP_GetCaps(pre, &mut caps);
        let _ = HidD_FreePreparsedData(pre);
        (status == HIDP_STATUS_SUCCESS).then_some((attrs, caps))
    }
}

/// Every present HID interface path (NUL-terminated UTF-16).
fn hid_paths() -> Vec<Vec<u16>> {
    let mut out = Vec::new();
    unsafe {
        let guid = HidD_GetHidGuid();
        let Ok(set) = SetupDiGetClassDevsW(
            Some(&guid),
            PCWSTR::null(),
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        ) else {
            return out;
        };
        let mut index = 0;
        loop {
            let mut iface = SP_DEVICE_INTERFACE_DATA {
                cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInterfaces(set, None, &guid, index, &mut iface).is_err() {
                break;
            }
            index += 1;
            let mut needed = 0u32;
            let _ = SetupDiGetDeviceInterfaceDetailW(set, &iface, None, 0, Some(&mut needed), None);
            if needed < 8 {
                continue;
            }
            // u32-aligned buffer large enough for the variable-length detail struct.
            let mut buf = vec![0u32; (needed as usize).div_ceil(4)];
            let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(set, &iface, Some(detail), needed, None, None)
                .is_err()
            {
                continue;
            }
            let start = std::ptr::addr_of!((*detail).DevicePath) as *const u16;
            let max = (needed as usize - 4) / 2;
            let mut path = Vec::with_capacity(max + 1);
            for i in 0..max {
                let c = *start.add(i);
                if c == 0 {
                    break;
                }
                path.push(c);
            }
            path.push(0);
            out.push(path);
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
    }
    out
}

impl HidKeyboard {
    /// Find the built-in keyboard's 9-byte feature collection. Opening and querying
    /// attributes/caps sends nothing to the keyboard.
    pub fn open() -> Result<Self, KbdError> {
        for path in hid_paths() {
            let lower = String::from_utf16_lossy(&path[..path.len() - 1]).to_ascii_lowercase();
            if !VENDOR_IDS
                .iter()
                .any(|v| lower.contains(&format!("vid_{v:04x}")))
            {
                continue;
            }
            let Some(probe) = open_path(&path, 0) else {
                continue;
            };
            let Some((attrs, caps)) = attributes_and_caps(&probe) else {
                continue;
            };
            drop(probe);
            if !VENDOR_IDS.contains(&attrs.VendorID)
                || !ITE_PRODUCT_IDS.contains(&attrs.ProductID)
                || caps.FeatureReportByteLength as usize != FEATURE_LEN
            {
                continue;
            }
            let Some(handle) = open_path(&path, GENERIC_READ.0 | GENERIC_WRITE.0) else {
                return Err(last_error("open keyboard collection"));
            };
            let mut kb = HidKeyboard {
                handle,
                info: DeviceInfo {
                    vendor_id: attrs.VendorID,
                    product_id: attrs.ProductID,
                    path: lower,
                    firmware: None,
                },
                caps,
            };
            kb.info.firmware = kb.firmware_version().ok();
            return Ok(kb);
        }
        Err(KbdError::NotFound)
    }

    fn set_feature(&self, report: &Report) -> Result<(), KbdError> {
        let ok =
            unsafe { HidD_SetFeature(self.handle.0, report.as_ptr() as _, FEATURE_LEN as u32) };
        if ok {
            Ok(())
        } else {
            Err(last_error("HidD_SetFeature"))
        }
    }

    fn get_feature(&self) -> Result<Report, KbdError> {
        let mut buf: Report = [0; FEATURE_LEN];
        let ok =
            unsafe { HidD_GetFeature(self.handle.0, buf.as_mut_ptr() as _, FEATURE_LEN as u32) };
        if ok {
            Ok(buf)
        } else {
            Err(last_error("HidD_GetFeature"))
        }
    }

    fn firmware_version(&self) -> Result<String, KbdError> {
        self.set_feature(&proto::encode_get_version())?;
        sleep(Duration::from_millis(10));
        Ok(proto::parse_version(&self.get_feature()?))
    }

    fn overlapped_io(
        &self,
        write: Option<&[u8]>,
        read: Option<&mut [u8]>,
    ) -> Result<usize, KbdError> {
        unsafe {
            let event = Handle(
                CreateEventW(None, true, false, PCWSTR::null())
                    .map_err(|e| KbdError::Io(e.to_string()))?,
            );
            let mut ov = OVERLAPPED {
                hEvent: event.0,
                ..Default::default()
            };
            let started = match (write, read) {
                (Some(data), _) => WriteFile(self.handle.0, Some(data), None, Some(&mut ov)),
                (None, Some(buf)) => ReadFile(self.handle.0, Some(buf), None, Some(&mut ov)),
                (None, None) => return Ok(0),
            };
            if let Err(e) = started {
                if e.code() != windows::Win32::Foundation::ERROR_IO_PENDING.to_hresult() {
                    return Err(KbdError::Io(e.to_string()));
                }
            }
            if WaitForSingleObject(event.0, IO_TIMEOUT_MS) != WAIT_OBJECT_0 {
                let _ = CancelIo(self.handle.0);
                let mut n = 0u32;
                let _ = GetOverlappedResult(self.handle.0, &ov, &mut n, true);
                return Err(KbdError::Io("keyboard did not answer within 1 s".into()));
            }
            let mut n = 0u32;
            GetOverlappedResult(self.handle.0, &ov, &mut n, false)
                .map_err(|e| KbdError::Io(e.to_string()))?;
            Ok(n as usize)
        }
    }

    fn require_picture_pipe(&self) -> Result<(), KbdError> {
        let need = (MATRIX_CHUNK + 1) as u16;
        if self.caps.OutputReportByteLength != need || self.caps.InputReportByteLength != need {
            return Err(KbdError::Io(format!(
                "this collection has {}-byte input / {}-byte output reports, not 65; Custom pictures unsupported",
                self.caps.InputReportByteLength, self.caps.OutputReportByteLength
            )));
        }
        Ok(())
    }
}

impl KeyboardBackend for HidKeyboard {
    fn label(&self) -> &'static str {
        "REAL"
    }

    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn apply(&mut self, settings: &LightSettings) -> Result<Verified, KbdError> {
        let report = proto::encode_set(settings)?;
        self.set_feature(&report)?;
        sleep(SETTLE);
        let read = self.read()?;
        verify(settings, read)
    }

    fn read(&mut self) -> Result<LightSettings, KbdError> {
        self.set_feature(&proto::encode_get())?;
        let answer = self.get_feature()?;
        proto::parse_effect(&answer)
            .ok_or_else(|| KbdError::Io(format!("unrecognised 0x88 answer {answer:02X?}")))
    }

    fn read_custom(&mut self, slot: u8) -> Result<[u8; MATRIX_LEN], KbdError> {
        self.require_picture_pipe()?;
        self.set_feature(&proto::encode_get_picture(slot)?)?;
        let _ = self.get_feature()?;
        sleep(Duration::from_millis(25));
        let mut out = [0u8; MATRIX_LEN];
        for chunk in out.chunks_exact_mut(MATRIX_CHUNK) {
            let mut buf = [0u8; MATRIX_CHUNK + 1];
            let n = self.overlapped_io(None, Some(&mut buf))?;
            if n != buf.len() {
                return Err(KbdError::Io(format!("short picture read ({n} bytes)")));
            }
            chunk.copy_from_slice(&buf[1..]);
            sleep(Duration::from_millis(25));
        }
        Ok(out)
    }

    fn write_custom(&mut self, slot: u8, matrix: &[u8; MATRIX_LEN]) -> Result<(), KbdError> {
        self.require_picture_pipe()?;
        self.set_feature(&proto::encode_set_picture(slot)?)?;
        sleep(SETTLE);
        for chunk in proto::picture_chunks(matrix) {
            let n = self.overlapped_io(Some(&chunk), None)?;
            if n != chunk.len() {
                return Err(KbdError::Io(format!("short picture write ({n} bytes)")));
            }
            sleep(Duration::from_millis(100));
        }
        let back = self.read_custom(slot)?;
        if &back != matrix {
            return Err(KbdError::Io(
                "Custom picture readback differs from what was written".into(),
            ));
        }
        Ok(())
    }
}
