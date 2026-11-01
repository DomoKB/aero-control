//! Built-in Control Center modes and the YB factory fan curve.
//!
//! Quiet, Normal, Gaming, and Max are firmware switches, not curves. Control
//! Center ships one default custom curve per model family. This crate only
//! includes the YB table.

use crate::curve::{CurvePoint, FanCurve};
use crate::mode::FanMode;
use crate::profile::{Config, CurvePointToml, Profile};
use crate::scale::percent_to_raw;

#[derive(Debug, Clone)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub provisional: bool,
    pub notes: &'static str,
    pub kind: PresetKind,
}

#[derive(Debug, Clone)]
pub enum PresetKind {
    Firmware(FanMode),
    Fixed { duty_percent: u8 },
    Curve(FanCurve),
}

pub fn presets() -> Vec<Preset> {
    vec![
        Preset {
            id: "normal",
            name: "Normal",
            provisional: false,
            notes: "Control Center's default thermal profile: firmware automatic fan control.",
            kind: PresetKind::Firmware(FanMode::Normal),
        },
        Preset {
            id: "quiet",
            name: "Quiet",
            provisional: false,
            notes: "Control Center's quiet profile: firmware fan control with the GPU thermal target set to quiet.",
            kind: PresetKind::Firmware(FanMode::Quiet),
        },
        Preset {
            id: "gaming",
            name: "Gaming",
            provisional: false,
            notes: "Control Center's gaming profile: the firmware boost fan curve and NvPowerConfig on.",
            kind: PresetKind::Firmware(FanMode::Gaming),
        },
        Preset {
            id: "max",
            name: "Max",
            provisional: false,
            notes: "Both fans at raw 229 (100%).",
            kind: PresetKind::Firmware(FanMode::Max),
        },
        Preset {
            id: "fixed",
            name: "Fixed",
            provisional: false,
            notes: "One duty for the CPU fan and the GPU fan. Default 50%. The floor is 35% unless expert mode is on.",
            kind: PresetKind::Fixed { duty_percent: 50 },
        },
        curve_preset("quiet-curve", "Quiet curve", false, QUIET_NOTES, QUIET_YB_PERCENT),
        Preset {
            id: "factory",
            name: "Factory",
            provisional: false,
            notes: FACTORY_NOTES,
            kind: PresetKind::Curve(factory_curve()),
        },
    ]
}

/// Control Center `setDefault_Click` for model names containing `yb`.
/// Confirmed by `GetFanIndexValue` 0..14 on AERO 17 YB P77YB BIOS FB10.
pub const FACTORY_YB_PERCENT: &[(u8, u8)] = &[
    (0, 25),
    (50, 30),
    (53, 35),
    (56, 40),
    (59, 45),
    (62, 50),
    (65, 55),
    (68, 60),
    (71, 65),
    (74, 70),
    (77, 75),
    (80, 80),
    (83, 85),
    (86, 90),
    (89, 100),
];

/// Quiet-first curve from the 2026-10-31 curvelab run on this AERO 17 YB.
/// Under full load the CPU holds about 90°C at any fan speed from 25% to 55%,
/// so extra fan buys work, not temperature: 55% does all of it, 45% about 97%.
/// Idle sits near 57°C at 25%. 50% from 85°C and 80% at 90°C are the curve rules.
pub const QUIET_YB_PERCENT: &[(u8, u8)] = &[
    (0, 25),
    (65, 30),
    (72, 35),
    (78, 40),
    (82, 45),
    (85, 50),
    (88, 55),
    (90, 80),
];

const QUIET_NOTES: &str = "Reference quiet-first curve measured on an AERO 17 YB: 25% at idle, 50 to 55% under sustained full load, where the CPU held about 90°C regardless of fan speed. These measurements do not describe every detected laptop.";

/// Raw duty bytes read from that laptop. They are `floor(percent * 229 / 100)`.
pub const FACTORY_YB_CHIP_RAW: &[u8] = &[
    57, 68, 80, 91, 103, 114, 125, 137, 148, 160, 171, 183, 194, 206, 229,
];

const FACTORY_NOTES: &str = "Reference Control Center curve verified on an AERO 17 YB (P77YB, BIOS FB10), including its exact raw duty bytes. It is not automatically a factory curve for other detected models.";

fn curve_preset(
    id: &'static str,
    name: &'static str,
    provisional: bool,
    notes: &'static str,
    points: &[(u8, u8)],
) -> Preset {
    Preset {
        id,
        name,
        provisional,
        notes,
        kind: PresetKind::Curve(curve_from_percent(points)),
    }
}

pub fn quiet_curve() -> FanCurve {
    curve_from_percent(QUIET_YB_PERCENT)
}

pub fn factory_curve() -> FanCurve {
    let points = FACTORY_YB_PERCENT
        .iter()
        .map(|(temp, percent)| CurvePoint {
            temp_c: *temp,
            duty_raw: gcc_floor_raw(*percent),
        })
        .collect();
    FanCurve::try_new(points).expect("factory curve must pass validation")
}

/// Control Center truncates percent to the 0–229 duty byte.
pub fn gcc_floor_raw(percent: u8) -> u8 {
    let percent = u16::from(percent.min(100));
    ((percent * 229) / 100) as u8
}

pub fn curve_from_percent(points: &[(u8, u8)]) -> FanCurve {
    let points = points
        .iter()
        .map(|(temp, percent)| CurvePoint {
            temp_c: *temp,
            duty_raw: percent_to_raw(*percent),
        })
        .collect();
    FanCurve::try_new(points).expect("built-in curve must pass validation")
}

pub fn builtin_config() -> Config {
    Config {
        active: "Normal".to_string(),
        ac_profile: Some("Gaming".to_string()),
        dc_profile: Some("Quiet".to_string()),
        profiles: presets()
            .into_iter()
            .map(|preset| Profile {
                name: if preset.id == "factory" {
                    "Deep Control".to_string()
                } else {
                    preset.name.to_string()
                },
                fan_mode: match &preset.kind {
                    PresetKind::Firmware(mode) => *mode,
                    PresetKind::Fixed { .. } => FanMode::Fixed,
                    PresetKind::Curve(_) => FanMode::Custom,
                },
                fixed_duty_percent: match preset.kind {
                    PresetKind::Fixed { duty_percent } => Some(duty_percent),
                    _ => None,
                },
                fixed_duty_raw: None,
                curve: match &preset.kind {
                    PresetKind::Curve(curve) => curve
                        .points()
                        .iter()
                        .map(|point| CurvePointToml {
                            temp_c: point.temp_c,
                            duty_percent: Some(crate::raw_to_percent(point.duty_raw)),
                            duty_raw: Some(point.duty_raw),
                        })
                        .collect(),
                    _ => Vec::new(),
                },
                curve_pairs: Vec::new(),
                dynamic_boost: None,
                nv_power_config: None,
                nv_thermal_target: None,
                ai_boost: None,
                charge_limit: None,
                charge_policy: None,
                expert_mode: false,
                provisional: preset.provisional,
                notes: preset.notes.to_string(),
                cpu_pl1_w: None,
                cpu_pl2_w: None,
                cpu_power_preset: None,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_curves_are_valid_and_confirmed() {
        let presets = presets();
        let curves: Vec<_> = presets
            .iter()
            .filter(|preset| matches!(preset.kind, PresetKind::Curve(_)))
            .collect();
        assert_eq!(curves.len(), 2);
        assert!(curves.iter().all(|preset| !preset.provisional));
        assert_eq!(quiet_curve().points().len(), QUIET_YB_PERCENT.len());
        assert!(presets.iter().all(|preset| !preset.provisional));
        let curve = factory_curve();
        let preset = presets
            .iter()
            .find(|preset| preset.id == "factory")
            .unwrap();
        assert!(matches!(&preset.kind, PresetKind::Curve(stored) if *stored == curve));
        assert_eq!(curve.points().len(), 15);
        assert!(curve
            .points()
            .iter()
            .all(|point| !(point.temp_c == 0 && point.duty_raw == 0)));
        for ((temp, percent), chip) in FACTORY_YB_PERCENT.iter().zip(FACTORY_YB_CHIP_RAW) {
            assert_eq!(gcc_floor_raw(*percent), *chip, "{temp}°C");
            assert!(curve
                .points()
                .iter()
                .any(|point| point.temp_c == *temp && point.duty_raw == *chip));
        }
        assert_eq!(percent_to_raw(30), 69);
        assert_eq!(gcc_floor_raw(30), 68);
        assert_eq!(curve.points()[1].duty_raw, 68);
        assert_eq!(curve.points()[1].temp_c, 50);
    }

    #[test]
    fn builtin_config_round_trips_through_toml() {
        let config = builtin_config();
        let parsed = Config::from_toml(&config.to_toml().unwrap()).unwrap();
        assert_eq!(parsed.ac_profile.as_deref(), Some("Gaming"));
        assert_eq!(parsed.dc_profile.as_deref(), Some("Quiet"));
        let deep = parsed.profile("Deep Control").unwrap();
        assert_eq!(deep.fan_mode, FanMode::Custom);
        assert!(!deep.provisional);
        let curve = deep.fan_curve().unwrap();
        assert_eq!(curve, factory_curve());
    }
}
