//! Allow-list of firmware methods aero-control calls.
//!
//! Method ids and parameter layouts come from Gigabyte's `GB_WMIACPI_Get` /
//! `GB_WMIACPI_Set` MOF. Parameter names keep the MOF spelling (`Temperture`).
//! Anything not listed here cannot be executed.

use crate::error::WmiError;
use crate::value::{WmiNumber, WmiOutput};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    Get,
    Set,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    U8,
    U16,
}

impl Width {
    fn bytes(self) -> usize {
        match self {
            Self::U8 => 1,
            Self::U16 => 2,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::U8 => "uint8",
            Self::U16 => "uint16",
        }
    }
}

/// One allowed method. Every in-parameter is a `uint8`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodSpec {
    pub name: &'static str,
    pub block: Block,
    pub id: u32,
    pub inputs: &'static [&'static str],
    pub outputs: &'static [(&'static str, Width)],
}

/// Refused by name even though the allow-list already excludes them.
pub const NEVER_CALL: &[&str] = &["TurnOffFan"];

const DATA_U8: &[(&str, Width)] = &[("Data", Width::U8)];
const DATA_U16: &[(&str, Width)] = &[("Data", Width::U16)];
const DATA_OUT: &[(&str, Width)] = &[("DataOut", Width::U8)];

const fn get(name: &'static str, id: u32, outputs: &'static [(&'static str, Width)]) -> MethodSpec {
    MethodSpec {
        name,
        block: Block::Get,
        id,
        inputs: &[],
        outputs,
    }
}

const fn set(name: &'static str, id: u32, outputs: &'static [(&'static str, Width)]) -> MethodSpec {
    MethodSpec {
        name,
        block: Block::Set,
        id,
        inputs: &["Data"],
        outputs,
    }
}

pub static GETTERS: &[MethodSpec] = &[
    get("GetCPUFanDuty", 70, DATA_U8),
    get("GetGPUFanDuty", 71, DATA_U8),
    get("GetStepFanStatus", 103, DATA_U16),
    MethodSpec {
        name: "GetFanIndexValue",
        block: Block::Get,
        id: 104,
        inputs: &["Index"],
        outputs: &[("Temperture", Width::U8), ("Value", Width::U8)],
    },
    get("GetFixedFanStatus", 106, DATA_U16),
    get("GetFixedFanSpeed", 107, DATA_U16),
    get("GetFanAdjustStatus", 112, DATA_U8),
    get("GetAutoFanStatus", 113, DATA_U8),
    get("GetFanSpeed", 125, DATA_U8),
    get("GetWhisperMode", 133, DATA_U8),
    get("getCpuTemp", 225, DATA_U16),
    get("getGpuTemp1", 226, DATA_U16),
    get("getGpuTemp2", 227, DATA_U16),
    get("getRpm1", 228, DATA_U16),
    get("getRpm2", 229, DATA_U16),
];

pub static SETTERS: &[MethodSpec] = &[
    set("SetGPUFanDuty", 71, DATA_OUT),
    set("SetNvPowerConfig", 81, &[]),
    set("SetSuperQuiet", 88, &[]),
    set("SetCurrentFanStep", 102, DATA_OUT),
    set("SetStepFanStatus", 103, DATA_OUT),
    MethodSpec {
        name: "SetFanIndexValue",
        block: Block::Set,
        id: 104,
        inputs: &["Index", "Temperture", "Value"],
        outputs: &[],
    },
    set("SetFixedFanStatus", 106, DATA_OUT),
    set("SetFixedFanSpeed", 107, DATA_OUT),
    set("SetFanAdjustStatus", 112, DATA_OUT),
    set("SetAutoFanStatus", 113, DATA_OUT),
    set("SetDynamicBoostStatus", 231, &[]),
];

/// wmiacpi hands the input buffer to the ACPI method as a buffer argument;
/// short buffers are padded to one DWORD, as Gigabyte's own tool does.
const MIN_INPUT: usize = 4;

pub fn find_getter(name: &str) -> Option<&'static MethodSpec> {
    GETTERS.iter().find(|spec| spec.name == name)
}

pub fn find_setter(name: &str) -> Option<&'static MethodSpec> {
    SETTERS.iter().find(|spec| spec.name == name)
}

/// In-parameters packed in MOF order. All are `uint8`, so no padding between them.
pub fn pack_input(spec: &MethodSpec, params: &[(&str, u8)]) -> Result<Vec<u8>, WmiError> {
    let mut buf = spec
        .inputs
        .iter()
        .map(|name| {
            params
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| *value)
                .ok_or(WmiError::MissingParam {
                    method: spec.name.to_string(),
                    name,
                })
        })
        .collect::<Result<Vec<u8>, _>>()?;
    buf.resize(buf.len().max(MIN_INPUT), 0);
    Ok(buf)
}

/// Out-parameters read in MOF order with natural alignment, little-endian.
pub fn decode_output(spec: &MethodSpec, bytes: &[u8]) -> Result<WmiOutput, WmiError> {
    let mut output = WmiOutput::new();
    let mut offset = 0usize;
    for &(name, width) in spec.outputs {
        let size = width.bytes();
        offset = offset.next_multiple_of(size);
        let field = bytes
            .get(offset..offset + size)
            .ok_or_else(|| WmiError::CallFailed {
                method: spec.name.to_string(),
                detail: format!(
                    "{} output bytes, {name} needs {}",
                    bytes.len(),
                    offset + size
                ),
            })?;
        let value = match width {
            Width::U8 => WmiNumber::U8(field[0]),
            Width::U16 => WmiNumber::U16(u16::from_le_bytes([field[0], field[1]])),
        };
        output.insert(name, value);
        offset += size;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_per_block_and_turn_off_fan_is_absent() {
        for table in [GETTERS, SETTERS] {
            for (i, spec) in table.iter().enumerate() {
                assert!(
                    table[i + 1..].iter().all(|other| other.id != spec.id),
                    "{}",
                    spec.name
                );
            }
        }
        assert!(GETTERS.iter().all(|spec| spec.block == Block::Get));
        assert!(SETTERS.iter().all(|spec| spec.block == Block::Set));
        assert!(find_setter("TurnOffFan").is_none());
        assert_eq!(find_setter("SetFanIndexValue").unwrap().id, 104);
        assert_eq!(find_getter("getCpuTemp").unwrap().id, 225);
    }

    #[test]
    fn data_setter_packs_one_byte_padded_to_a_dword() {
        let spec = find_setter("SetAutoFanStatus").unwrap();
        assert_eq!(pack_input(spec, &[("Data", 1)]).unwrap(), [1, 0, 0, 0]);
        let spec = find_setter("SetFixedFanSpeed").unwrap();
        assert_eq!(pack_input(spec, &[("Data", 229)]).unwrap(), [229, 0, 0, 0]);
    }

    #[test]
    fn fan_index_setter_packs_index_temp_value_in_mof_order() {
        let spec = find_setter("SetFanIndexValue").unwrap();
        let params = [("Value", 0x50), ("Index", 3), ("Temperture", 0x2d)];
        assert_eq!(pack_input(spec, &params).unwrap(), [3, 0x2d, 0x50, 0]);
    }

    #[test]
    fn missing_input_is_an_error() {
        let spec = find_setter("SetFanIndexValue").unwrap();
        let err = pack_input(spec, &[("Index", 1)]).unwrap_err();
        assert!(matches!(
            err,
            WmiError::MissingParam {
                name: "Temperture",
                ..
            }
        ));
    }

    #[test]
    fn getters_send_a_zero_dword() {
        let spec = find_getter("getCpuTemp").unwrap();
        assert_eq!(pack_input(spec, &[]).unwrap(), [0, 0, 0, 0]);
        let spec = find_getter("GetFanIndexValue").unwrap();
        assert_eq!(pack_input(spec, &[("Index", 14)]).unwrap(), [14, 0, 0, 0]);
    }

    #[test]
    fn decodes_u8_and_u16_getters() {
        let cpu = decode_output(find_getter("getCpuTemp").unwrap(), &[0x4b, 0, 0, 0]).unwrap();
        assert_eq!(cpu.get("Data"), Some(WmiNumber::U16(75)));
        let rpm = decode_output(find_getter("getRpm1").unwrap(), &[0x0c, 0x80, 0, 0]).unwrap();
        assert_eq!(rpm.get("Data"), Some(WmiNumber::U16(0x800c)));
        let auto = decode_output(
            find_getter("GetAutoFanStatus").unwrap(),
            &[1, 0xff, 0xff, 0xff],
        )
        .unwrap();
        assert_eq!(auto.get("Data"), Some(WmiNumber::U8(1)));
        let point =
            decode_output(find_getter("GetFanIndexValue").unwrap(), &[45, 80, 0, 0]).unwrap();
        assert_eq!(point.get("Temperture"), Some(WmiNumber::U8(45)));
        assert_eq!(point.get("Value"), Some(WmiNumber::U8(80)));
    }

    #[test]
    fn short_output_is_an_error() {
        let spec = find_getter("getCpuTemp").unwrap();
        assert!(decode_output(spec, &[0x4b]).is_err());
        let spec = find_setter("SetSuperQuiet").unwrap();
        assert!(decode_output(spec, &[]).unwrap().properties.is_empty());
    }
}
