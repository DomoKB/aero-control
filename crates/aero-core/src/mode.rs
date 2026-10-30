//! Firmware fan-mode scripts.
//!
//! Each script is a list of `GB_WMIACPI_Set` calls. It does not read
//! temperatures. Sources and the provisional lines are described on the
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
    pub provisional: bool,
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
/// the mode we're about to enter. Clearing `SetFanAdjustStatus` is a defensive
/// extra and is marked provisional in the notes.
fn clears(super_quiet: u8) -> Vec<PlannedWrite> {
    vec![
        data("SetFanAdjustStatus", 0),
        data("SetSuperQuiet", super_quiet),
        data("SetCurrentFanStep", 0),
        data("SetFixedFanStatus", 0),
        data("SetStepFanStatus", 0),
    ]
}

pub fn normal_script() -> ModeScript {
    let mut writes = clears(0);
    writes.push(data("SetAutoFanStatus", 1));
    ModeScript {
        name: "normal",
        provisional: false,
        notes: "Firmware automatic mode. The tail (fixed off, step off, auto on) is the scope's restore sequence. SetCurrentFanStep(0) is the §5.1 bugfix. SetFanAdjustStatus(0) and SetSuperQuiet(0) drop leftover overrides.",
        writes,
    }
}

pub fn quiet_script() -> ModeScript {
    let mut writes = clears(0);
    // Auto stays on so a missing SuperQuiet method cannot leave the fans with
    // no governor. The Aero 16 "Silence" trace turns auto off as part of a
    // larger power-mode sequence (whisper + thermal target) that we do not send.
    writes.push(data("SetAutoFanStatus", 1));
    writes.push(data("SetSuperQuiet", 1));
    ModeScript {
        name: "quiet",
        provisional: true,
        notes: "PROVISIONAL. Quiet and Super Quiet are the same SetSuperQuiet switch (the scope groups them). Auto is left on, which differs from the Aero 16 Silence trace, because that trace also changes whisper mode and the thermal target.",
        writes,
    }
}

pub fn gaming_script() -> ModeScript {
    let mut script = normal_script();
    script.name = "gaming";
    script.provisional = true;
    script.notes = "PROVISIONAL. The Aero 16 Gaming trace uses the same fan flags as Normal. On that machine the gaming change is Dynamic Boost and NvPowerConfig, which are not applied yet. AERO 17 YB DeepFan may use a different fan script; the probe is how to check. The Gaming curve preset is a separate custom table.";
    script
}

pub fn fixed_script(duty_raw: u8) -> ModeScript {
    ModeScript {
        name: "fixed",
        provisional: false,
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
        provisional: false,
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
        provisional: false,
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

    #[test]
    fn normal_tail_matches_the_scope_restore_sequence() {
        let script = normal_script();
        let tail: Vec<_> = script
            .writes
            .iter()
            .rev()
            .take(3)
            .map(|write| (write.method, write.params.clone()))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        assert_eq!(
            tail,
            vec![
                ("SetFixedFanStatus", WriteParams::Data(0)),
                ("SetStepFanStatus", WriteParams::Data(0)),
                ("SetAutoFanStatus", WriteParams::Data(1)),
            ]
        );
        assert!(script
            .writes
            .iter()
            .any(|write| write.method == "SetCurrentFanStep"));
    }

    #[test]
    fn gaming_fan_writes_match_normal_until_the_sku_is_probed() {
        assert_eq!(gaming_script().writes, normal_script().writes);
        assert!(gaming_script().provisional);
        assert!(!normal_script().provisional);
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

    #[test]
    fn quiet_requests_super_quiet_and_leaves_auto_on() {
        let script = quiet_script();
        assert!(script.provisional);
        assert!(script.writes.iter().any(|write| {
            write.method == "SetSuperQuiet" && write.params == WriteParams::Data(1)
        }));
        assert_eq!(
            script
                .writes
                .last()
                .map(|write| (write.method, write.params.clone())),
            Some(("SetSuperQuiet", WriteParams::Data(1)))
        );
        assert!(script.writes.iter().any(|write| {
            write.method == "SetAutoFanStatus" && write.params == WriteParams::Data(1)
        }));
    }
}
