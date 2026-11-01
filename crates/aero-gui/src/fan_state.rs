//! Recognizes the target YB's exposed fan-mode configuration using getters.
//! A step-mode flag alone cannot distinguish a custom table from an automatic
//! capped mode, so it deliberately remains unknown after a restart.

use aero_core::FanMode;
use aero_wmi::AcpiReader;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecognizedFanState {
    pub mode: FanMode,
    pub fixed_raw: Option<u8>,
    pub note: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FanStateError {
    AmbiguousStep,
    Unknown(String),
}

impl std::fmt::Display for FanStateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AmbiguousStep => formatter.write_str("Step control is enabled, but the firmware does not identify whether this is a custom curve or a capped automatic mode."),
            Self::Unknown(message) => formatter.write_str(message),
        }
    }
}

impl From<String> for FanStateError {
    fn from(message: String) -> Self {
        Self::Unknown(message)
    }
}

impl From<&str> for FanStateError {
    fn from(message: &str) -> Self {
        Self::Unknown(message.into())
    }
}

pub fn read<R: AcpiReader>(
    reader: &mut R,
    nv_thermal_target: &Result<u32, String>,
) -> Result<RecognizedFanState, FanStateError> {
    let auto = flag("GetAutoFanStatus", value(reader, "GetAutoFanStatus")?)?;
    let fixed = flag("GetFixedFanStatus", value(reader, "GetFixedFanStatus")?)?;
    let step = flag("GetStepFanStatus", value(reader, "GetStepFanStatus")?)?;
    let quiet = flag("GetNvThermalTarget", nv_thermal_target.clone()?)?;
    let mode = match (auto, fixed, step, quiet) {
        (false, false, false, false) => FanMode::Normal,
        (false, false, false, true) => FanMode::Quiet,
        (true, false, false, false) => FanMode::Gaming,
        (false, true, true, false) => {
            let cpu = value(reader, "GetFixedFanSpeed")?;
            let gpu = value(reader, "GetGPUFanDuty")?;
            if cpu != gpu || !(1..=229).contains(&cpu) {
                return Err(format!(
                    "Fixed fan duties are not a recognized pair (CPU {cpu}, GPU {gpu})."
                )
                .into());
            }
            let raw = cpu as u8;
            return Ok(RecognizedFanState {
                mode: if raw == 229 { FanMode::Max } else { FanMode::Fixed },
                fixed_raw: Some(raw),
                note: (raw == 229).then_some("Both fans are fixed at 100%; the firmware cannot distinguish Max from Fixed at 100%."),
            });
        }
        (false, false, true, false) => {
            return Err(FanStateError::AmbiguousStep);
        }
        _ => {
            return Err("The fan flags and thermal profile do not match a recognized mode.".into())
        }
    };
    Ok(RecognizedFanState {
        mode,
        fixed_raw: None,
        note: None,
    })
}

fn value<R: AcpiReader>(reader: &mut R, method: &str) -> Result<u32, String> {
    reader
        .get(method, &[])
        .map_err(|error| error.to_string())?
        .get("Data")
        .and_then(|value| value.as_u32())
        .ok_or_else(|| format!("{method} returned no Data"))
}

fn flag(method: &str, value: u32) -> Result<bool, String> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(format!("{method} returned an unknown flag value {value}.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aero_wmi::{MockAcpi, NoPacer, WmiError, WmiOutput};

    fn detect(reader: &mut MockAcpi) -> Result<RecognizedFanState, FanStateError> {
        let thermal = value(reader, "GetNvThermalTarget");
        read(reader, &thermal)
    }

    #[test]
    fn recognizes_applied_yb_modes_from_fresh_readback_without_writes() {
        for mode in [
            FanMode::Normal,
            FanMode::Quiet,
            FanMode::Gaming,
            FanMode::Fixed,
            FanMode::Max,
        ] {
            let mut original = MockAcpi::aero_17_yb_fixture();
            aero_core::apply_mode(&mut original, &mut NoPacer, mode, 120, false).unwrap();
            // Only the controller state survives; detection has no remembered UI mode.
            let mut restarted = original.clone();
            restarted.calls.clear();
            let state = detect(&mut restarted).unwrap();
            assert_eq!(state.mode, mode);
            assert_eq!(
                state.fixed_raw,
                match mode {
                    FanMode::Fixed => Some(120),
                    FanMode::Max => Some(229),
                    _ => None,
                }
            );
            assert!(restarted
                .calls
                .iter()
                .all(|call| call.class == aero_wmi::GET_CLASS));
        }
    }

    #[test]
    fn gaming_remains_recognizable_after_the_gpu_request_clears() {
        let mut reader = MockAcpi::aero_17_yb_fixture();
        aero_core::apply_mode(&mut reader, &mut NoPacer, FanMode::Quiet, 0, false).unwrap();
        assert_eq!(detect(&mut reader).unwrap().mode, FanMode::Quiet);
        reader.nv_thermal_target = 0;
        assert_eq!(detect(&mut reader).unwrap().mode, FanMode::Normal);
        reader.auto = 1;
        assert_eq!(detect(&mut reader).unwrap().mode, FanMode::Gaming);
        reader.nv_power_config = 1;
        assert_eq!(detect(&mut reader).unwrap().mode, FanMode::Gaming);
    }

    #[test]
    fn ambiguous_step_mode_and_invalid_or_unequal_duties_remain_unknown() {
        let mut reader = MockAcpi::aero_17_yb_fixture();
        reader.auto = 0;
        reader.step = 1;
        assert!(detect(&mut reader).is_err());
        assert_eq!(reader.calls_named("GetFanIndexValue").count(), 0);
        reader.fixed = 1;
        reader.fixed_speed = 120;
        reader.gpu_duty = 121;
        assert!(detect(&mut reader).is_err());
        reader.fixed_speed = 0;
        reader.gpu_duty = 0;
        assert!(detect(&mut reader).is_err());
        reader.fixed_speed = 230;
        reader.gpu_duty = 230;
        assert!(detect(&mut reader).is_err());
        reader.auto = 2;
        assert!(detect(&mut reader).is_err());
    }

    #[test]
    fn fixed_at_full_speed_is_reported_as_max_with_an_ambiguity_note() {
        let mut reader = MockAcpi::aero_17_yb_fixture();
        aero_core::apply_mode(&mut reader, &mut NoPacer, FanMode::Fixed, 229, false).unwrap();
        let state = detect(&mut reader).unwrap();
        assert_eq!(state.mode, FanMode::Max);
        assert_eq!(state.fixed_raw, Some(229));
        assert!(state.note.is_some());
    }

    #[test]
    fn failed_fan_or_thermal_getters_do_not_invent_a_mode() {
        struct FailingReader;
        impl AcpiReader for FailingReader {
            fn class_status(&mut self, _: &str) -> Result<aero_wmi::ClassStatus, WmiError> {
                Ok(aero_wmi::ClassStatus::Present)
            }
            fn get(&mut self, method: &str, _: &[(&str, u8)]) -> Result<WmiOutput, WmiError> {
                Err(WmiError::MethodNotFound {
                    method: method.into(),
                })
            }
        }
        assert!(read(&mut FailingReader, &Ok(0)).is_err());
        let mut reader = MockAcpi::aero_17_yb_fixture();
        reader.auto = 0;
        assert!(read(&mut reader, &Err("Thermal read failed".into())).is_err());
        assert!(read(&mut reader, &Ok(2)).is_err());
    }
}
