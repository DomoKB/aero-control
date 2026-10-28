//! In-memory embedded controller.
//!
//! The mock records every call. RPM getters return the byte-swapped tach, the
//! same quirk the real EC shows on older AERO boards, so the probe's decoder
//! is exercised without hardware.

#[cfg(feature = "writer")]
use std::collections::HashSet;

use crate::sku::{SkuInfo, SkuSource};

use crate::catalog::{find_getter, GETTERS, SETTERS};
use crate::cim::{cim_type_name, CIM_UINT8};
use crate::diag::{AcpiDiag, MethodSignature, ParamField};
use crate::error::WmiError;
use crate::read::{refuse_setter, AcpiReader, ClassStatus};
use crate::value::{WmiNumber, WmiOutput};
use crate::{FAN_CURVE_SLOTS, GET_CLASS, SET_CLASS};

#[cfg(feature = "writer")]
use crate::write::{refuse_forbidden, AcpiWriter, SetOutcome, WriteParams};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedCall {
    pub class: &'static str,
    pub method: String,
    pub params: Vec<(String, u8)>,
}

#[derive(Debug, Clone)]
pub struct MockAcpi {
    pub cpu_temp: u16,
    pub gpu_temp1: u16,
    pub gpu_temp2: u16,
    /// Real RPM. `getRpm1` returns this value with its bytes swapped.
    pub rpm1: u16,
    pub rpm2: u16,
    pub auto: u8,
    pub fixed: u8,
    pub step: u8,
    pub super_quiet: u8,
    pub fan_adjust: u8,
    pub current_step: u8,
    pub custom_cap: u8,
    pub fixed_speed: u8,
    pub gpu_duty: u8,
    pub curve: [(u8, u8); FAN_CURVE_SLOTS],
    pub calls: Vec<RecordedCall>,
    pub mof_present: bool,
    /// The next N `GetFanIndexValue` calls return duty + 1, then behave.
    pub corrupt_fan_index_reads: u32,
    #[cfg(feature = "writer")]
    pub throw_setters: HashSet<String>,
    #[cfg(feature = "writer")]
    pub missing_setters: HashSet<String>,
    #[cfg(feature = "writer")]
    pub panic_on_set: Option<String>,
}

impl Default for MockAcpi {
    fn default() -> Self {
        Self::aero_17_yb_fixture()
    }
}

impl MockAcpi {
    /// A stand-in for the AERO 17 YB. Temperatures and RPMs are fixture
    /// values, not a recording from that laptop.
    pub fn aero_17_yb_fixture() -> Self {
        let mut curve = [(0, 0); FAN_CURVE_SLOTS];
        // Probe fixture table. Not the YB factory curve.
        let points = [(45, 80), (60, 103), (75, 137), (85, 183), (90, 229)];
        for (slot, point) in curve.iter_mut().zip(points) {
            *slot = point;
        }
        Self {
            cpu_temp: 61,
            gpu_temp1: 57,
            gpu_temp2: 54,
            rpm1: 3200,
            rpm2: 2800,
            auto: 1,
            fixed: 0,
            step: 0,
            super_quiet: 0,
            fan_adjust: 0,
            current_step: 0,
            custom_cap: 0,
            fixed_speed: 0,
            gpu_duty: 0,
            curve,
            calls: Vec::new(),
            mof_present: true,
            corrupt_fan_index_reads: 0,
            #[cfg(feature = "writer")]
            throw_setters: HashSet::new(),
            #[cfg(feature = "writer")]
            missing_setters: HashSet::new(),
            #[cfg(feature = "writer")]
            panic_on_set: None,
        }
    }

    pub fn calls_named<'a>(
        &'a self,
        method: &'a str,
    ) -> impl Iterator<Item = &'a RecordedCall> + 'a {
        self.calls.iter().filter(move |call| call.method == method)
    }

    fn record(&mut self, class: &'static str, method: &str, params: &[(&str, u8)]) {
        self.calls.push(RecordedCall {
            class,
            method: method.to_string(),
            params: params
                .iter()
                .map(|(name, value)| ((*name).to_string(), *value))
                .collect(),
        });
    }

    fn output_u8(&self, name: &str, value: u8) -> WmiOutput {
        let mut output = WmiOutput::new();
        output.insert(name, WmiNumber::U8(value));
        output
    }

    fn output_u16(&self, name: &str, value: u16) -> WmiOutput {
        let mut output = WmiOutput::new();
        output.insert(name, WmiNumber::U16(value));
        output
    }

    fn zeros_for(&self, method: &str) -> Result<WmiOutput, WmiError> {
        let spec = find_getter(method).ok_or_else(|| WmiError::MethodNotFound {
            method: method.to_string(),
        })?;
        let mut output = WmiOutput::new();
        for name in spec.outputs {
            output.insert(name, WmiNumber::U8(0));
        }
        Ok(output)
    }

    fn fan_index(&mut self, params: &[(&str, u8)]) -> Result<WmiOutput, WmiError> {
        let index = param(params, "Index").ok_or(WmiError::MissingParam {
            method: "GetFanIndexValue".to_string(),
            name: "Index",
        })?;
        if index as usize >= FAN_CURVE_SLOTS {
            return Err(WmiError::IndexOutOfRange { index });
        }
        let (temp, mut duty) = self.curve[index as usize];
        if self.corrupt_fan_index_reads > 0 {
            self.corrupt_fan_index_reads -= 1;
            duty = duty.wrapping_add(1);
        }
        let mut output = WmiOutput::new();
        output.insert("Temperture", WmiNumber::U8(temp));
        output.insert("Value", WmiNumber::U8(duty));
        Ok(output)
    }
}

fn param(params: &[(&str, u8)], name: &str) -> Option<u8> {
    params
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| *value)
}

impl AcpiReader for MockAcpi {
    fn class_status(&mut self, class: &str) -> Result<ClassStatus, WmiError> {
        if self.mof_present && (class == GET_CLASS || class == SET_CLASS) {
            Ok(ClassStatus::Present)
        } else {
            Ok(ClassStatus::Missing {
                detail: format!("{class} is not in the mock MOF"),
            })
        }
    }

    fn get(&mut self, method: &str, params: &[(&str, u8)]) -> Result<WmiOutput, WmiError> {
        refuse_setter(method)?;
        self.record(GET_CLASS, method, params);
        let output = match method {
            "getCpuTemp" => self.output_u16("Data", self.cpu_temp),
            "getGpuTemp1" => self.output_u16("Data", self.gpu_temp1),
            "getGpuTemp2" => self.output_u16("Data", self.gpu_temp2),
            "getRpm1" => self.output_u16("Data", self.rpm1.rotate_left(8)),
            "getRpm2" => self.output_u16("Data", self.rpm2.rotate_left(8)),
            "GetAutoFanStatus" => self.output_u8("Data", self.auto),
            "GetFixedFanStatus" => self.output_u8("Data", self.fixed),
            "GetStepFanStatus" => self.output_u8("Data", self.step),
            "GetFanAdjustStatus" => self.output_u8("Data", self.fan_adjust),
            "GetSuperQuiet" => self.output_u8("Data", self.super_quiet),
            "GetFanCustomStatus" => self.output_u8("Data", self.custom_cap),
            "GetFixedFanSpeed" => self.output_u16("Data", u16::from(self.fixed_speed)),
            "GetCPUFanDuty" => self.output_u8("Data", self.fixed_speed),
            "GetGPUFanDuty" => self.output_u8("Data", self.gpu_duty),
            "GetFanSpeed" => self.output_u8("Data", self.fixed_speed),
            "GetWhisperMode" => self.output_u8("Data", 0),
            "GetFanIndexValue" => self.fan_index(params)?,
            other => self.zeros_for(other)?,
        };
        Ok(output)
    }
}

#[cfg(feature = "writer")]
impl AcpiWriter for MockAcpi {
    fn set(&mut self, method: &str, params: &WriteParams) -> Result<SetOutcome, WmiError> {
        refuse_forbidden(method)?;
        let pairs = params.pairs();
        let pair_refs: Vec<(&str, u8)> =
            pairs.iter().map(|(name, value)| (*name, *value)).collect();
        self.record(SET_CLASS, method, &pair_refs);

        if self.panic_on_set.as_deref() == Some(method) {
            panic!("mock panic on {method}");
        }
        if self.missing_setters.contains(method) {
            return Err(WmiError::MethodNotFound {
                method: method.to_string(),
            });
        }

        match (method, params) {
            ("SetAutoFanStatus", WriteParams::Data(value)) => self.auto = *value,
            ("SetFixedFanStatus", WriteParams::Data(value)) => self.fixed = *value,
            ("SetStepFanStatus", WriteParams::Data(value)) => self.step = *value,
            ("SetFixedFanSpeed", WriteParams::Data(value)) => self.fixed_speed = *value,
            ("SetGPUFanDuty", WriteParams::Data(value)) => self.gpu_duty = *value,
            ("SetCurrentFanStep", WriteParams::Data(value)) => self.current_step = *value,
            ("SetSuperQuiet", WriteParams::Data(value)) => self.super_quiet = *value,
            ("SetFanAdjustStatus", WriteParams::Data(value)) => self.fan_adjust = *value,
            ("SetFanCustomStatus", WriteParams::Data(value)) => self.custom_cap = *value,
            ("SetFanIndexValue", WriteParams::FanPoint { index, temp, duty }) => {
                if *index as usize >= FAN_CURVE_SLOTS {
                    return Err(WmiError::IndexOutOfRange { index: *index });
                }
                self.curve[*index as usize] = (*temp, *duty);
            }
            _ => {
                return Err(WmiError::MethodNotFound {
                    method: method.to_string(),
                });
            }
        }

        let threw = self
            .throw_setters
            .contains(method)
            .then(|| "WBEM error (mock: write was still applied)".to_string());
        Ok(SetOutcome { threw })
    }
}

impl AcpiDiag for MockAcpi {
    fn method_signature(&mut self, class: &str, method: &str) -> Result<MethodSignature, WmiError> {
        let spec = GETTERS
            .iter()
            .chain(SETTERS.iter())
            .find(|spec| spec.name == method)
            .ok_or_else(|| WmiError::MethodNotFound {
                method: method.to_string(),
            })?;
        let field = |name: &str| ParamField {
            name: name.to_string(),
            cim_type: CIM_UINT8,
            cim_type_name: cim_type_name(CIM_UINT8).to_string(),
        };
        Ok(MethodSignature {
            class: class.to_string(),
            method: method.to_string(),
            instance_path: format!("mock:{class}.InstanceName=\"fixture\""),
            in_params: spec.inputs.iter().copied().map(field).collect(),
            out_params: spec.outputs.iter().copied().map(field).collect(),
        })
    }
}

impl SkuSource for MockAcpi {
    fn read_sku(&mut self) -> Result<SkuInfo, WmiError> {
        Ok(SkuInfo {
            manufacturer: Some("Gigabyte Technology Co., Ltd.".to_string()),
            model: Some("AERO 17 YB".to_string()),
            system_family: Some("AERO".to_string()),
            system_sku: Some("AERO 17 YB".to_string()),
            bios_version: None,
            bios_manufacturer: None,
            baseboard_product: None,
            source: "mock fixture; not read from hardware".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::{decode_rpm, RpmOrder};
    use crate::read::is_setter_name;

    #[test]
    fn reader_refuses_setters_and_turn_off_fan() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        let err = mock.get("SetAutoFanStatus", &[("Data", 1)]).unwrap_err();
        assert!(matches!(err, WmiError::Refused { .. }));
        let err = mock.get("TurnOffFan", &[("Data", 1)]).unwrap_err();
        assert!(matches!(err, WmiError::Refused { .. }));
        assert!(mock.calls.is_empty());
        assert!(is_setter_name("SetFanIndexValue"));
        assert!(!is_setter_name("getRpm1"));
    }

    #[test]
    fn fixture_rpm_is_byte_swapped_on_the_wire() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        let raw = mock
            .get("getRpm1", &[])
            .unwrap()
            .get("Data")
            .unwrap()
            .as_u32()
            .unwrap();
        assert_ne!(raw, 3200);
        let reading = decode_rpm(raw, RpmOrder::AutoPreferSwap);
        assert_eq!(reading.rpm, 3200);
        assert_eq!(reading.chose, "swapped-only-plausible");
    }

    #[cfg(feature = "writer")]
    #[test]
    fn turn_off_fan_is_refused_by_the_writer_too() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        let err = mock.set("TurnOffFan", &WriteParams::Data(1)).unwrap_err();
        assert!(matches!(err, WmiError::Refused { .. }));
        assert!(mock.calls.is_empty());
    }
}
