//! Windows `GB_WMIACPI` backend.
//!
//! Calls go to an **instance** path (`__PATH` from `CreateInstanceEnum` /
//! `GetInstances`), never the class path. The AERO 17 YB returns
//! `0x8004102F` (`WBEM_E_INVALID_METHOD_PARAMETERS`) for class-path calls.
//! Methods whose signature has no non-system in-params are executed with a
//! NULL in-params pointer. When in-params exist, each value is coerced from
//! the signature's CIM type and `Put` with that CIM type.
//!
//! `WindowsReader` only ever executes methods on `GB_WMIACPI_Get`.
//! `WindowsWriter` is the only type that touches `GB_WMIACPI_Set`.
#![allow(unsafe_code)]

use std::collections::HashMap;

use windows::core::{BSTR, HSTRING, PCWSTR};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemContext, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
    WBEM_GENERIC_FLAG_TYPE, WBEM_INFINITE,
};
use wmi::result_enumerator::IWbemClassWrapper;
use wmi::{COMLibrary, Variant, WMIConnection};

use crate::cim::{cim_type_name, coerce_byte, pass_null_inparams, CoercedInt};
use crate::diag::{AcpiDiag, MethodSignature, ParamField};
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
    /// Class name → `__PATH` (or `__RELPATH`) of the first instance.
    instance_paths: HashMap<String, String>,
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
            instance_paths: HashMap::new(),
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
        invoke(
            &self.wmi,
            &mut self.instance_paths,
            GET_CLASS,
            method,
            params,
        )
    }
}

impl AcpiDiag for WindowsReader {
    fn method_signature(&mut self, class: &str, method: &str) -> Result<MethodSignature, WmiError> {
        let path = instance_path(&self.wmi, &mut self.instance_paths, class)?;
        let class_obj = load_class(&self.wmi, class)?;
        let (in_params, out_params) = signature_fields(&class_obj, method)?;
        Ok(MethodSignature {
            class: class.to_string(),
            method: method.to_string(),
            instance_path: path,
            in_params,
            out_params,
        })
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
        match invoke(
            &self.inner.wmi,
            &mut self.inner.instance_paths,
            SET_CLASS,
            method,
            &refs,
        ) {
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

fn invoke(
    conn: &WMIConnection,
    paths: &mut HashMap<String, String>,
    class: &str,
    method: &str,
    params: &[(&str, u8)],
) -> Result<WmiOutput, WmiError> {
    let path = instance_path(conn, paths, class)?;
    let class_obj = load_class(conn, class)?;
    let (in_fields, _) = signature_fields(&class_obj, method)?;
    let in_params = if pass_null_inparams(in_fields.len()) {
        None
    } else {
        let sig = in_signature(&class_obj, method)?;
        let spawned = unsafe { sig.SpawnInstance(0) }.map_err(|err| WmiError::CallFailed {
            method: method.to_string(),
            detail: format!("SpawnInstance on {path}: {err}"),
        })?;
        for (name, value) in params {
            let Some(field) = in_fields
                .iter()
                .find(|field| field.name.eq_ignore_ascii_case(name))
            else {
                continue;
            };
            put_byte(&spawned, name, *value, field.cim_type).map_err(|err| {
                WmiError::CallFailed {
                    method: method.to_string(),
                    detail: format!("Put {name} ({}) on {path}: {err}", field.cim_type_name),
                }
            })?;
        }
        Some(spawned)
    };
    let mut output = None;
    let exec = unsafe {
        conn.svc.ExecMethod(
            &BSTR::from(path.as_str()),
            &BSTR::from(method),
            WBEM_GENERIC_FLAG_TYPE(0),
            None::<&IWbemContext>,
            in_params.as_ref(),
            Some(&mut output),
            None,
        )
    };
    exec.map_err(|err| map_method_error(method, format!("{err} (instance {path})")))?;
    convert_output(method, output.map(IWbemClassWrapper::new))
}

fn instance_path(
    conn: &WMIConnection,
    paths: &mut HashMap<String, String>,
    class: &str,
) -> Result<String, WmiError> {
    if let Some(path) = paths.get(class) {
        return Ok(path.clone());
    }
    let path =
        enumerate_instance_path(conn, class).or_else(|_| query_instance_path(conn, class))?;
    paths.insert(class.to_string(), path.clone());
    Ok(path)
}

fn enumerate_instance_path(conn: &WMIConnection, class: &str) -> Result<String, WmiError> {
    let flags = WBEM_GENERIC_FLAG_TYPE(WBEM_FLAG_FORWARD_ONLY.0 | WBEM_FLAG_RETURN_IMMEDIATELY.0);
    let enumerator = unsafe {
        conn.svc
            .CreateInstanceEnum(&BSTR::from(class), flags, None::<&IWbemContext>)
    }
    .map_err(|err| WmiError::CallFailed {
        method: class.to_string(),
        detail: format!("CreateInstanceEnum: {err}"),
    })?;
    let mut batch = [None];
    let mut returned = 0u32;
    let hr = unsafe { enumerator.Next(WBEM_INFINITE, &mut batch, &mut returned) };
    if hr.is_err() || returned == 0 {
        return Err(WmiError::CallFailed {
            method: class.to_string(),
            detail: format!("CreateInstanceEnum returned no {class} instance ({hr:?})"),
        });
    }
    let object = batch[0].take().ok_or_else(|| WmiError::CallFailed {
        method: class.to_string(),
        detail: format!("CreateInstanceEnum returned an empty {class} slot"),
    })?;
    read_path_property(&object, "__PATH")
        .or_else(|| read_path_property(&object, "__RELPATH"))
        .ok_or_else(|| WmiError::CallFailed {
            method: class.to_string(),
            detail: format!("{class} instance has no __PATH or __RELPATH"),
        })
}

fn query_instance_path(conn: &WMIConnection, class: &str) -> Result<String, WmiError> {
    let query = format!("SELECT __PATH, __RELPATH, InstanceName FROM {class}");
    let rows = conn
        .raw_query::<HashMap<String, Variant>>(&query)
        .map_err(|err| WmiError::CallFailed {
            method: class.to_string(),
            detail: err.to_string(),
        })?;
    let row = rows.first().ok_or_else(|| WmiError::CallFailed {
        method: class.to_string(),
        detail: format!("{class} query returned no instances"),
    })?;
    row_string(row, "__PATH")
        .or_else(|| row_string(row, "__Path"))
        .or_else(|| row_string(row, "__RELPATH"))
        .or_else(|| row_string(row, "__Relpath"))
        .or_else(|| {
            row_string(row, "InstanceName").map(|name| format!("{class}.InstanceName=\"{name}\""))
        })
        .ok_or_else(|| WmiError::CallFailed {
            method: class.to_string(),
            detail: format!("{class} row had no path property"),
        })
}

fn load_class(conn: &WMIConnection, class: &str) -> Result<IWbemClassObject, WmiError> {
    let mut object = None;
    unsafe {
        conn.svc.GetObject(
            &BSTR::from(class),
            WBEM_GENERIC_FLAG_TYPE(0),
            None::<&IWbemContext>,
            Some(&mut object),
            None,
        )
    }
    .map_err(|err| WmiError::ClassMissing {
        class: class.to_string(),
        detail: err.to_string(),
    })?;
    object.ok_or_else(|| WmiError::ClassMissing {
        class: class.to_string(),
        detail: "GetObject returned no class".to_string(),
    })
}

fn signature_fields(
    class_obj: &IWbemClassObject,
    method: &str,
) -> Result<(Vec<ParamField>, Vec<ParamField>), WmiError> {
    let (in_sig, out_sig) = method_signatures(class_obj, method)?;
    let in_params = match &in_sig {
        Some(sig) => fields_of(sig)?,
        None => Vec::new(),
    };
    let out_params = match &out_sig {
        Some(sig) => fields_of(sig)?,
        None => Vec::new(),
    };
    Ok((in_params, out_params))
}

fn in_signature(class_obj: &IWbemClassObject, method: &str) -> Result<IWbemClassObject, WmiError> {
    let (in_sig, _) = method_signatures(class_obj, method)?;
    in_sig.ok_or_else(|| WmiError::CallFailed {
        method: method.to_string(),
        detail: "method has no in-signature to spawn".to_string(),
    })
}

fn method_signatures(
    class_obj: &IWbemClassObject,
    method: &str,
) -> Result<(Option<IWbemClassObject>, Option<IWbemClassObject>), WmiError> {
    let mut in_sig = None;
    let mut out_sig = None;
    let name = wide(method);
    unsafe {
        class_obj.GetMethod(
            PCWSTR::from_raw(name.as_ptr()),
            0,
            &mut in_sig,
            &mut out_sig,
        )
    }
    .map_err(|err| map_method_error(method, err))?;
    Ok((in_sig, out_sig))
}

fn fields_of(sig: &IWbemClassObject) -> Result<Vec<ParamField>, WmiError> {
    let wrapper = IWbemClassWrapper::new(sig.clone());
    let names = wrapper
        .list_properties()
        .map_err(|err| WmiError::CallFailed {
            method: "signature".to_string(),
            detail: err.to_string(),
        })?;
    let mut fields = Vec::new();
    for name in names {
        let cim_type = cim_type_of(sig, &name).unwrap_or(0);
        fields.push(ParamField {
            cim_type_name: cim_type_name(cim_type).to_string(),
            name,
            cim_type,
        });
    }
    Ok(fields)
}

fn cim_type_of(object: &IWbemClassObject, name: &str) -> Result<i32, WmiError> {
    let wide_name = wide(name);
    let mut value = VARIANT::default();
    let mut cim_type = 0i32;
    unsafe {
        object.Get(
            PCWSTR::from_raw(wide_name.as_ptr()),
            0,
            &mut value,
            Some(&mut cim_type),
            None,
        )
    }
    .map_err(|err| WmiError::CallFailed {
        method: name.to_string(),
        detail: err.to_string(),
    })?;
    Ok(cim_type)
}

fn put_byte(object: &IWbemClassObject, name: &str, value: u8, cim_type: i32) -> Result<(), String> {
    let coerced = coerce_byte(value, cim_type);
    let variant = match coerced {
        CoercedInt::Ui1(byte) => VARIANT::from(byte),
        CoercedInt::I2(number) => VARIANT::from(number),
        CoercedInt::I4(number) => VARIANT::from(number),
    };
    let wide_name = wide(name);
    unsafe {
        object.Put(
            PCWSTR::from_raw(wide_name.as_ptr()),
            0,
            &variant,
            crate::cim::cim_base(cim_type),
        )
    }
    .map_err(|err| err.to_string())
}

fn read_path_property(object: &IWbemClassObject, name: &str) -> Option<String> {
    let wide_name = wide(name);
    let mut value = VARIANT::default();
    let mut cim_type = 0i32;
    unsafe {
        object
            .Get(
                PCWSTR::from_raw(wide_name.as_ptr()),
                0,
                &mut value,
                Some(&mut cim_type),
                None,
            )
            .ok()?;
    }
    match Variant::from_variant(&value).ok()? {
        Variant::String(text) if !text.is_empty() => Some(text),
        _ => None,
    }
}

fn wide(text: &str) -> HSTRING {
    HSTRING::from(text)
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
