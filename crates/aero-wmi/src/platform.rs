//! Windows backend: the `GB_WMIACPI_Get` / `GB_WMIACPI_Set` firmware blocks
//! called directly through advapi32's WMI data-block API. No MOF or CIM class
//! has to be registered; the process must be elevated.
//!
//! `WindowsReader` only opens `GB_WMIACPI_Get`. `WindowsWriter` is the only
//! type that opens `GB_WMIACPI_Set`.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::ptr;

use crate::catalog::{
    decode_output, find_getter, pack_input, Block, MethodSpec, Width, GETTERS, SETTERS,
};
use crate::diag::{AcpiDiag, MethodSignature, ParamField};
use crate::error::WmiError;
use crate::read::{refuse_setter, AcpiReader, ClassStatus};
use crate::sku::{SkuInfo, SkuSource};
use crate::value::WmiOutput;
use crate::{GET_CLASS, SET_CLASS};

#[cfg(feature = "writer")]
use crate::catalog::find_setter;
#[cfg(feature = "writer")]
use crate::write::{refuse_forbidden, AcpiWriter, SetOutcome, WriteParams};

#[repr(C)]
struct Guid {
    d1: u32,
    d2: u16,
    d3: u16,
    d4: [u8; 8],
}

const GB_TAIL: [u8; 8] = [0x00, 0xA0, 0xC9, 0x06, 0x29, 0x10, 0x00, 0x00];
const GET_GUID: Guid = Guid {
    d1: 0xABBC0F6F,
    d2: 0x8EA1,
    d3: 0x11d1,
    d4: GB_TAIL,
};
const SET_GUID: Guid = Guid {
    d1: 0xABBC0F75,
    d2: 0x8EA1,
    d3: 0x11d1,
    d4: GB_TAIL,
};

const WMIGUID_QUERY: u32 = 0x0001;
const WMIGUID_EXECUTE: u32 = 0x0010;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_WMI_GUID_NOT_FOUND: u32 = 4200;
const ERROR_WMI_ITEMID_NOT_FOUND: u32 = 4202;

const INSTANCE_CANDIDATES: &[&str] = &["DCK", "0", "1", "2", "3"];
const HKEY_LOCAL_MACHINE: isize = 0x8000_0002_u32 as i32 as isize;
const RRF_RT_REG_SZ: u32 = 0x2;

#[link(name = "advapi32", kind = "raw-dylib")]
extern "system" {
    fn WmiOpenBlock(guid: *const Guid, access: u32, handle: *mut *mut c_void) -> u32;
    fn WmiCloseBlock(handle: *mut c_void) -> u32;
    fn WmiQueryAllDataW(handle: *mut c_void, size: *mut u32, buf: *mut c_void) -> u32;
    fn WmiExecuteMethodW(
        handle: *mut c_void,
        instance: *const u16,
        method: u32,
        in_size: u32,
        in_buf: *mut c_void,
        out_size: *mut u32,
        out_buf: *mut c_void,
    ) -> u32;
    fn RegGetValueW(
        key: isize,
        subkey: *const u16,
        value: *const u16,
        flags: u32,
        ty: *mut u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> i32;
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn map_error(method: &str, rc: u32) -> WmiError {
    match rc {
        ERROR_ACCESS_DENIED => WmiError::AccessDenied,
        ERROR_WMI_GUID_NOT_FOUND => WmiError::FirmwareMissing,
        ERROR_WMI_ITEMID_NOT_FOUND => WmiError::MethodNotFound {
            method: method.to_string(),
        },
        _ => WmiError::CallFailed {
            method: method.to_string(),
            detail: format!("Win32 error {rc}"),
        },
    }
}

struct WmiBlock(*mut c_void);

impl WmiBlock {
    fn open(guid: &Guid, class: &str) -> Result<Self, WmiError> {
        let mut handle = ptr::null_mut();
        let rc = unsafe { WmiOpenBlock(guid, WMIGUID_QUERY | WMIGUID_EXECUTE, &mut handle) };
        if rc == 0 {
            Ok(Self(handle))
        } else {
            Err(map_error(class, rc))
        }
    }

    fn instance_names(&self) -> Vec<String> {
        let mut size = 0u32;
        unsafe { WmiQueryAllDataW(self.0, &mut size, ptr::null_mut()) };
        if size == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; size as usize];
        if unsafe { WmiQueryAllDataW(self.0, &mut size, buf.as_mut_ptr().cast()) } != 0 {
            return Vec::new();
        }
        parse_instance_names(&buf).unwrap_or_default()
    }

    fn execute(
        &self,
        instance: &[u16],
        spec: &MethodSpec,
        input: &mut [u8],
    ) -> Result<Vec<u8>, u32> {
        let mut out = vec![0u8; 64];
        loop {
            let mut out_size = out.len() as u32;
            let rc = unsafe {
                WmiExecuteMethodW(
                    self.0,
                    instance.as_ptr(),
                    spec.id,
                    input.len() as u32,
                    input.as_mut_ptr().cast(),
                    &mut out_size,
                    out.as_mut_ptr().cast(),
                )
            };
            match rc {
                0 => {
                    out.truncate(out_size as usize);
                    return Ok(out);
                }
                ERROR_INSUFFICIENT_BUFFER if out_size as usize > out.len() => {
                    out.resize(out_size as usize, 0);
                }
                rc => return Err(rc),
            }
        }
    }
}

impl Drop for WmiBlock {
    fn drop(&mut self) {
        unsafe { WmiCloseBlock(self.0) };
    }
}

/// Instance names from a `WNODE_ALL_DATA`: instance count at +52, offset of
/// the name-offset table at +56; each name is a byte length then UTF-16.
fn parse_instance_names(buf: &[u8]) -> Option<Vec<String>> {
    let u32_at = |o: usize| -> Option<usize> {
        Some(u32::from_le_bytes(buf.get(o..o + 4)?.try_into().ok()?) as usize)
    };
    let (count, table) = (u32_at(52)?, u32_at(56)?);
    (0..count)
        .map(|i| {
            let at = u32_at(table + 4 * i)?;
            let len = u16::from_le_bytes(buf.get(at..at + 2)?.try_into().ok()?) as usize;
            let units: Vec<u16> = buf
                .get(at + 2..at + 2 + len)?
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16(&units).ok()
        })
        .collect()
}

pub struct WindowsReader {
    get: WmiBlock,
    instance: Option<String>,
}

impl WindowsReader {
    pub fn connect() -> Result<Self, WmiError> {
        Ok(Self {
            get: WmiBlock::open(&GET_GUID, GET_CLASS)?,
            instance: None,
        })
    }

    fn instance(&mut self) -> Result<String, WmiError> {
        if let Some(name) = &self.instance {
            return Ok(name.clone());
        }
        let name = match self.get.instance_names().into_iter().next() {
            Some(name) => name,
            None => self.probe_instance()?,
        };
        self.instance = Some(name.clone());
        Ok(name)
    }

    fn probe_instance(&self) -> Result<String, WmiError> {
        let spec = find_getter("getCpuTemp").expect("getCpuTemp is allow-listed");
        let mut last = ERROR_WMI_GUID_NOT_FOUND;
        for uid in INSTANCE_CANDIDATES {
            let name = format!(r"ACPI\PNP0C14\{uid}_0");
            match self
                .get
                .execute(&wide(&name), spec, &mut pack_input(spec, &[])?)
            {
                Ok(_) => return Ok(name),
                Err(ERROR_ACCESS_DENIED) => return Err(WmiError::AccessDenied),
                Err(rc) => last = rc,
            }
        }
        Err(map_error(GET_CLASS, last))
    }

    fn call(
        &mut self,
        block: Option<&WmiBlock>,
        spec: &MethodSpec,
        params: &[(&str, u8)],
    ) -> Result<Vec<u8>, WmiError> {
        let mut input = pack_input(spec, params)?;
        let instance = wide(&self.instance()?);
        let block = block.unwrap_or(&self.get);
        block
            .execute(&instance, spec, &mut input)
            .map_err(|rc| map_error(spec.name, rc))
    }
}

impl AcpiReader for WindowsReader {
    fn class_status(&mut self, class: &str) -> Result<ClassStatus, WmiError> {
        let guid = match class {
            GET_CLASS => &GET_GUID,
            SET_CLASS => &SET_GUID,
            _ => {
                return Ok(ClassStatus::Missing {
                    detail: format!("{class} is not a GB_WMIACPI block"),
                })
            }
        };
        Ok(match WmiBlock::open(guid, class) {
            Ok(_) => ClassStatus::Present,
            Err(err) => ClassStatus::Missing {
                detail: err.to_string(),
            },
        })
    }

    fn get(&mut self, method: &str, params: &[(&str, u8)]) -> Result<WmiOutput, WmiError> {
        refuse_setter(method)?;
        let spec = find_getter(method).ok_or(WmiError::Refused {
            method: method.to_string(),
            reason: "not in the allow-list",
        })?;
        decode_output(spec, &self.call(None, spec, params)?)
    }
}

impl AcpiDiag for WindowsReader {
    fn method_signature(&mut self, class: &str, method: &str) -> Result<MethodSignature, WmiError> {
        let block = if class == SET_CLASS {
            Block::Set
        } else {
            Block::Get
        };
        let spec = GETTERS
            .iter()
            .chain(SETTERS)
            .find(|spec| spec.block == block && spec.name == method)
            .ok_or_else(|| WmiError::MethodNotFound {
                method: method.to_string(),
            })?;
        let field = |name: &str, width: Width| ParamField {
            name: name.to_string(),
            ty: width.name().to_string(),
        };
        Ok(MethodSignature {
            class: class.to_string(),
            method: method.to_string(),
            instance: self.instance()?,
            in_params: spec
                .inputs
                .iter()
                .map(|name| field(name, Width::U8))
                .collect(),
            out_params: spec
                .outputs
                .iter()
                .map(|(name, width)| field(name, *width))
                .collect(),
        })
    }
}

fn bios_string(value: &str) -> Option<String> {
    let key = wide(r"HARDWARE\DESCRIPTION\System\BIOS");
    let value = wide(value);
    let mut buf = [0u16; 256];
    let mut size = std::mem::size_of_val(&buf) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if rc != 0 {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1);
    let text = String::from_utf16_lossy(&buf[..len]).trim().to_string();
    (!text.is_empty()).then_some(text)
}

impl SkuSource for WindowsReader {
    fn read_sku(&mut self) -> Result<SkuInfo, WmiError> {
        Ok(SkuInfo {
            manufacturer: bios_string("SystemManufacturer"),
            model: bios_string("SystemProductName"),
            system_family: bios_string("SystemFamily"),
            system_sku: bios_string("SystemSKU"),
            bios_version: bios_string("BIOSVersion"),
            bios_manufacturer: bios_string("BIOSVendor"),
            baseboard_product: bios_string("BaseBoardProduct"),
            source: r"HKLM\HARDWARE\DESCRIPTION\System\BIOS".to_string(),
        })
    }
}

#[cfg(feature = "writer")]
pub struct WindowsWriter {
    inner: WindowsReader,
    set: WmiBlock,
}

#[cfg(feature = "writer")]
impl WindowsWriter {
    pub fn connect() -> Result<Self, WmiError> {
        Ok(Self {
            inner: WindowsReader::connect()?,
            set: WmiBlock::open(&SET_GUID, SET_CLASS)?,
        })
    }
}

#[cfg(feature = "writer")]
impl AcpiReader for WindowsWriter {
    fn class_status(&mut self, class: &str) -> Result<ClassStatus, WmiError> {
        self.inner.class_status(class)
    }

    fn get(&mut self, method: &str, params: &[(&str, u8)]) -> Result<WmiOutput, WmiError> {
        self.inner.get(method, params)
    }
}

#[cfg(feature = "writer")]
impl AcpiWriter for WindowsWriter {
    fn set(&mut self, method: &str, params: &WriteParams) -> Result<SetOutcome, WmiError> {
        refuse_forbidden(method)?;
        let spec = find_setter(method).ok_or(WmiError::Refused {
            method: method.to_string(),
            reason: "not in the allow-list",
        })?;
        let pairs = params.pairs();
        match self.inner.call(Some(&self.set), spec, &pairs) {
            Ok(returned) => Ok(SetOutcome {
                threw: None,
                returned,
            }),
            // A delivered call the firmware rejected. The EC often still applies it.
            Err(WmiError::CallFailed { detail, .. }) => Ok(SetOutcome {
                threw: Some(detail),
                returned: Vec::new(),
            }),
            Err(other) => Err(other),
        }
    }
}

#[cfg(feature = "writer")]
impl SkuSource for WindowsWriter {
    fn read_sku(&mut self) -> Result<SkuInfo, WmiError> {
        self.inner.read_sku()
    }
}
