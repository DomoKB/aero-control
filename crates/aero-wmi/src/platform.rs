//! Windows `GB_WMIACPI` backend.
//!
//! `wmi` 0.14 calls `IWbemServices::ExecMethod` through
//! [`WMIConnection::exec_method_native_wrapper`]. That function loads the
//! class (`GetObject`), loads the method signature (`GetMethod`), spawns an
//! in-parameter instance, `Put`s each value, and executes. We pass
//! [`Variant::UI1`], which the crate converts with `VARIANT::from(u8)` —
//! `VT_UI1`, matching the MOF's `uint8` parameters.
//!
//! A class-level call is tried first. On failure the same call is repeated
//! against the first instance path. `method_class` stays the class name so
//! `GetMethod` still resolves.
//!
//! `WindowsReader` only ever executes methods on `GB_WMIACPI_Get`.
//! `WindowsWriter` is the only type that touches `GB_WMIACPI_Set`, and it is
//! compiled out unless the `writer` feature is on.
//!
//! The process has to be elevated. The crate initialises COM with
//! impersonation (`RPC_C_IMP_LEVEL_IMPERSONATE`). .NET's `EnablePrivileges`
//! flag has no separate switch here.

use std::collections::HashMap;

use wmi::result_enumerator::IWbemClassWrapper;
use wmi::{COMLibrary, Variant, WMIConnection};

use crate::error::WmiError;
use crate::read::{refuse_setter, AcpiReader, ClassStatus};
use crate::sku::{SkuInfo, SkuSource};
use crate::value::{WmiNumber, WmiOutput};
use crate::{GET_CLASS, SET_CLASS, WMI_NAMESPACE};

#[cfg(feature = "writer")]
use crate::write::{refuse_forbidden, AcpiWriter, SetOutcome, WriteParams};

pub struct WindowsReader {
    wmi: WMIConnection,
    cimv2: WMIConnection,
}

impl WindowsReader {
    pub fn connect() -> Result<Self, WmiError> {
        let com = COMLibrary::new().map_err(|err| WmiError::CallFailed {
            method: "CoInitialize".to_string(),
            detail: err.to_string(),
        })?;
        Ok(Self {
            wmi: connect(WMI_NAMESPACE, com)?,
            cimv2: connect(r"ROOT\CIMV2", com)?,
        })
    }
}

fn connect(namespace: &str, com: COMLibrary) -> Result<WMIConnection, WmiError> {
    WMIConnection::with_namespace_path(namespace, com).map_err(|err| WmiError::CallFailed {
        method: format!("ConnectServer {namespace}"),
        detail: err.to_string(),
    })
}

impl AcpiReader for WindowsReader {
    fn class_status(&mut self, class: &str) -> Result<ClassStatus, WmiError> {
        // A successful query means the class is registered, even with zero instances.
        let query = format!("SELECT InstanceName FROM {class}");
        match self.wmi.raw_query::<HashMap<String, Variant>>(&query) {
            Ok(_) => Ok(ClassStatus::Present),
            Err(err) => Ok(ClassStatus::Missing {
                detail: err.to_string(),
            }),
        }
    }

    fn get(&mut self, method: &str, params: &[(&str, u8)]) -> Result<WmiOutput, WmiError> {
        refuse_setter(method)?;
        exec(&self.wmi, GET_CLASS, method, params)
    }
}

impl SkuSource for WindowsReader {
    fn read_sku(&mut self) -> Result<SkuInfo, WmiError> {
        let mut info = SkuInfo {
            manufacturer: None,
            model: None,
            system_family: None,
            system_sku: None,
            bios_version: None,
            bios_manufacturer: None,
            baseboard_product: None,
            source: "Win32_ComputerSystem / Win32_BIOS / Win32_BaseBoard / MS_SystemInformation"
                .to_string(),
        };
        if let Some(row) = first_row(
            &self.cimv2,
            "SELECT Manufacturer, Model, SystemFamily, SystemSKUNumber FROM Win32_ComputerSystem",
        ) {
            info.manufacturer = row_string(&row, "Manufacturer");
            info.model = row_string(&row, "Model");
            info.system_family = row_string(&row, "SystemFamily");
            info.system_sku = row_string(&row, "SystemSKUNumber");
        }
        if let Some(row) = first_row(
            &self.cimv2,
            "SELECT SMBIOSBIOSVersion, Manufacturer FROM Win32_BIOS",
        ) {
            info.bios_version = row_string(&row, "SMBIOSBIOSVersion");
            info.bios_manufacturer = row_string(&row, "Manufacturer");
        }
        if let Some(row) = first_row(&self.cimv2, "SELECT Product FROM Win32_BaseBoard") {
            info.baseboard_product = row_string(&row, "Product");
        }
        if let Some(row) = first_row(&self.wmi, "SELECT * FROM MS_SystemInformation") {
            if info.baseboard_product.is_none() {
                info.baseboard_product = row_string(&row, "BaseBoardProduct");
            }
            if info.bios_version.is_none() {
                info.bios_version = row_string(&row, "BIOSVersion");
            }
            if info.system_sku.is_none() {
                info.system_sku = row_string(&row, "SystemSKU");
            }
        }
        Ok(info)
    }
}

#[cfg(feature = "writer")]
pub struct WindowsWriter {
    inner: WindowsReader,
}

#[cfg(feature = "writer")]
impl WindowsWriter {
    pub fn connect() -> Result<Self, WmiError> {
        Ok(Self {
            inner: WindowsReader::connect()?,
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
        let pairs = params.pairs();
        let refs: Vec<(&str, u8)> = pairs.iter().map(|(name, value)| (*name, *value)).collect();
        match exec(&self.inner.wmi, SET_CLASS, method, &refs) {
            Ok(_) => Ok(SetOutcome { threw: None }),
            // A delivered call that WMI rejected. The EC often still applies it.
            Err(WmiError::CallFailed { detail, .. }) => Ok(SetOutcome {
                threw: Some(detail),
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

fn exec(
    conn: &WMIConnection,
    class: &str,
    method: &str,
    params: &[(&str, u8)],
) -> Result<WmiOutput, WmiError> {
    let in_params = ui1_params(params);
    match conn.exec_method_native_wrapper(class, class, method, in_params.clone()) {
        Ok(output) => convert_output(method, output),
        Err(class_err) => match first_instance_path(conn, class)? {
            Some(path) => match conn.exec_method_native_wrapper(class, &path, method, in_params) {
                Ok(output) => convert_output(method, output),
                Err(err) => Err(map_method_error(method, err)),
            },
            None => Err(map_method_error(method, class_err)),
        },
    }
}

fn ui1_params(params: &[(&str, u8)]) -> HashMap<String, Variant> {
    params
        .iter()
        .map(|(name, value)| ((*name).to_string(), Variant::UI1(*value)))
        .collect()
}

fn convert_output(method: &str, output: Option<IWbemClassWrapper>) -> Result<WmiOutput, WmiError> {
    let Some(output) = output else {
        return Ok(WmiOutput::new());
    };
    let names = output
        .list_properties()
        .map_err(|err| WmiError::CallFailed {
            method: method.to_string(),
            detail: err.to_string(),
        })?;
    let mut collected = WmiOutput::new();
    for name in names {
        let Ok(value) = output.get_property(&name) else {
            continue;
        };
        if let Some(number) = number_from_variant(&value) {
            collected.insert(&name, number);
        }
    }
    Ok(collected)
}

fn number_from_variant(value: &Variant) -> Option<WmiNumber> {
    match value {
        Variant::UI1(v) => Some(WmiNumber::U8(*v)),
        Variant::UI2(v) => Some(WmiNumber::U16(*v)),
        Variant::UI4(v) => Some(WmiNumber::U32(*v)),
        Variant::I2(v) if *v >= 0 => Some(WmiNumber::U16(*v as u16)),
        Variant::I4(v) if (0..=i32::from(u16::MAX)).contains(v) => Some(WmiNumber::U16(*v as u16)),
        Variant::I4(v) if *v >= 0 => Some(WmiNumber::U32(*v as u32)),
        Variant::I4(v) => Some(WmiNumber::I32(*v)),
        Variant::I8(v) if *v >= 0 && *v <= i64::from(u32::MAX) => Some(WmiNumber::U32(*v as u32)),
        Variant::String(text) => parse_number_string(text),
        _ => None,
    }
}

fn parse_number_string(text: &str) -> Option<WmiNumber> {
    let value: u32 = text.trim().parse().ok()?;
    if value <= u32::from(u8::MAX) {
        Some(WmiNumber::U8(value as u8))
    } else if value <= u32::from(u16::MAX) {
        Some(WmiNumber::U16(value as u16))
    } else {
        Some(WmiNumber::U32(value))
    }
}

fn first_instance_path(conn: &WMIConnection, class: &str) -> Result<Option<String>, WmiError> {
    let query = format!("SELECT __Path FROM {class}");
    let rows = match conn.raw_query::<HashMap<String, Variant>>(&query) {
        Ok(rows) => rows,
        Err(_) => return Ok(None),
    };
    Ok(rows
        .first()
        .and_then(|row| row_string(row, "__Path").or_else(|| row_string(row, "__PATH"))))
}

fn first_row(conn: &WMIConnection, query: &str) -> Option<HashMap<String, Variant>> {
    conn.raw_query(query).ok().and_then(|mut rows: Vec<_>| {
        if rows.is_empty() {
            None
        } else {
            Some(rows.remove(0))
        }
    })
}

fn row_string(row: &HashMap<String, Variant>, name: &str) -> Option<String> {
    match row.get(name)? {
        Variant::String(text) if !text.is_empty() => Some(text.clone()),
        _ => None,
    }
}

fn map_method_error(method: &str, err: impl std::fmt::Display) -> WmiError {
    let detail = err.to_string();
    if is_missing(&detail) {
        WmiError::MethodNotFound {
            method: method.to_string(),
        }
    } else {
        WmiError::CallFailed {
            method: method.to_string(),
            detail,
        }
    }
}

fn is_missing(detail: &str) -> bool {
    let lower = detail.to_lowercase();
    lower.contains("not found")
        || lower.contains("invalid method")
        || lower.contains("invalid class")
        || lower.contains("80041002")
        || lower.contains("80041010")
}
