//! Firmware fan-mode scripts.
//!
//! Each script is a list of `GB_WMIACPI_Set` calls. It does not read
//! temperatures. Sources are described on the
//! scripts themselves and in `docs/SOURCES.md`.

use aero_wmi::WriteParams;
use serde::{Deserialize, Serialize};

use crate::curve::FanCurve;

/// Control Center fan modes.
///
/// `auto` is an alias of Normal (the scope's "Normal/Auto"). It is not the
/// Linux driver's "auto-maximum" mode. `super-quiet` and `silent` are aliases
/// of Quiet. `deep-control` is an alias of Custom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FanMode {
    #[serde(alias = "auto")]
    Normal,
    #[serde(alias = "silent", alias = "super-quiet", alias = "superquiet")]
    Quiet,
    Gaming,
    #[serde(alias = "turbo", alias = "full")]
    Max,
    Fixed,
    #[serde(alias = "deep-control", alias = "deepcontrol")]
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedWrite {
    pub method: &'static str,
    pub params: WriteParams,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeScript {
    pub name: &'static str,
    pub notes: &'static str,
    pub writes: Vec<PlannedWrite>,
}

fn data(method: &'static str, value: u8) -> PlannedWrite {
    PlannedWrite {
        method,
        params: WriteParams::Data(value),
    }
}

/// Clears that stop a leftover step, fixed duty, or quiet flag from overriding
/// the mode we're about to enter.
fn clears() -> Vec<PlannedWrite> {
    vec![
        data("SetFanAdjustStatus", 0),
        data("SetSuperQuiet", 0),
        data("SetCurrentFanStep", 0),
    ]
}

/// Control Center's DeepFan mode sequence for the AERO 17 YB, which has
/// bSuperQuiet, bSuperMode and bNvwhisper off and bNvPowerCfg on
/// (MainWindow.cs 1436-1445).
fn firmware_mode(auto: u8, thermal_target: u8, nv_power_config: u8) -> Vec<PlannedWrite> {
    let mut writes = clears();
    writes.extend([
        data("SetFixedFanStatus", 0),
        data("SetStepFanStatus", 0),
        data("SetAutoFanStatus", auto),
        data("SetNvThermalTarget", thermal_target),
        data("SetNvPowerConfig", nv_power_config),
    ]);
    writes
}

pub fn normal_script() -> ModeScript {
    ModeScript {
        name: "normal",
        notes: "Control Center normal_Click (DeepFan.cs 6801-6870): auto 0, SetQuietMode(0) which is SetNvThermalTarget (Helpers.cs 1399-1420), and SetNvPowerConfig(0) from FanAndOcTheme. The first three writes are extra clears.",
        writes: firmware_mode(0, 0, 0),
    }
}

pub fn quiet_script() -> ModeScript {
    ModeScript {
        name: "quiet",
        notes: "Control Center quiet_Click (DeepFan.cs 6730-6799): as Normal with SetNvThermalTarget(1). SetSuperQuiet is skipped on YB because bSuperQuiet is off.",
        writes: firmware_mode(0, 1, 0),
    }
}

pub fn gaming_script() -> ModeScript {
    ModeScript {
        name: "gaming",
        notes: "Control Center gaming_Click (DeepFan.cs 6872-6941): auto 1 (FANB, the EC boost curve) and SetNvPowerConfig(1) from FanAndOcTheme when smartfan is 2 (DeepFan.cs 7087+).",
        writes: firmware_mode(1, 0, 1),
    }
}

pub fn fixed_script(duty_raw: u8) -> ModeScript {
    ModeScript {
        name: "fixed",
        notes: "Scope Max sequence, with SetCurrentFanStep(0) from §5.1 and the Aero 16 Turbo trace. Duty 229 is 100% (scope and that trace).",
        writes: vec![
                data("SetSuperQuiet", 0),
            data("SetFanAdjustStatus", 0),
            data("SetCurrentFanStep", 0),
            data("SetAutoFanStatus", 0),
            data("SetFixedFanStatus", 0),
            data("SetStepFanStatus", 1),
            data("SetFixedFanStatus", 1),
            data("SetFixedFanSpeed", duty_raw),
            data("SetGPUFanDuty", duty_raw),
        ],
    }
}

pub fn fixed_duty_script(duty_raw: u8) -> ModeScript {
    ModeScript {
        name: "fixed duty",
        notes: "Duty change while fixed is already on: Control Center sends only 107 and 71, so no mode flag is cleared.",
        writes: vec![
            data("SetFixedFanSpeed", duty_raw),
            data("SetGPUFanDuty", duty_raw),
        ],
    }
}

pub fn custom_prefix() -> ModeScript {
    ModeScript {
        name: "custom",
        notes: "Full re-apply from §5.1: clear auto and fixed, SetCurrentFanStep(0), SetStepFanStatus(1), then all 15 slots. SetSuperQuiet(0) and SetFanAdjustStatus(0) are extra clears so a latched flag cannot hide the table.",
        writes: vec![
                data("SetAutoFanStatus", 0),
            data("SetFixedFanStatus", 0),
            data("SetSuperQuiet", 0),
            data("SetFanAdjustStatus", 0),
            data("SetCurrentFanStep", 0),
            data("SetStepFanStatus", 1),
        ],
    }
}

pub fn script_for(mode: FanMode, fixed_raw: u8) -> ModeScript {
    match mode {
        FanMode::Normal => normal_script(),
        FanMode::Quiet => quiet_script(),
        FanMode::Gaming => gaming_script(),
        FanMode::Max => fixed_script(229),
        FanMode::Fixed => fixed_script(fixed_raw),
        FanMode::Custom => custom_prefix(),
    }
}

pub fn fan_point_writes(curve: &FanCurve) -> Vec<PlannedWrite> {
    let slots = crate::planned_slots(curve);
    slots
        .into_iter()
        .enumerate()
        .map(|(index, (temp, duty))| PlannedWrite {
            method: "SetFanIndexValue",
            params: WriteParams::FanPoint {
                index: index as u8,
                temp,
                duty,
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tail(script: &ModeScript) -> Vec<(&'static str, WriteParams)> {
        script
            .writes
            .iter()
            .skip(3)
            .map(|write| (write.method, write.params.clone()))
            .collect()
    }

    fn cc_sequence(auto: u8, thermal: u8, nv: u8) -> Vec<(&'static str, WriteParams)> {
        vec![
            ("SetFixedFanStatus", WriteParams::Data(0)),
            ("SetStepFanStatus", WriteParams::Data(0)),
            ("SetAutoFanStatus", WriteParams::Data(auto)),
            ("SetNvThermalTarget", WriteParams::Data(thermal)),
            ("SetNvPowerConfig", WriteParams::Data(nv)),
        ]
    }

    #[test]
    fn firmware_modes_match_control_center_on_yb() {
        assert_eq!(tail(&normal_script()), cc_sequence(0, 0, 0));
        assert_eq!(tail(&gaming_script()), cc_sequence(1, 0, 1));
        assert_eq!(tail(&quiet_script()), cc_sequence(0, 1, 0));
        for script in [normal_script(), gaming_script(), quiet_script()] {
            assert!(!script.writes.iter().any(|write| {
                write.method == "SetSuperQuiet" && write.params != WriteParams::Data(0)
            }));
        }
    }

    #[test]
    fn max_ends_at_raw_229_on_both_fans() {
        let script = fixed_script(229);
        assert!(script.writes.iter().any(|write| {
            write.method == "SetFixedFanSpeed" && write.params == WriteParams::Data(229)
        }));
        assert!(script.writes.iter().any(|write| {
            write.method == "SetGPUFanDuty" && write.params == WriteParams::Data(229)
        }));
        assert!(script
            .writes
            .iter()
            .any(|write| write.method == "SetCurrentFanStep"));
    }
}
