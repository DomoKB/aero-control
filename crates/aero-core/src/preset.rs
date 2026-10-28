//! Built-in Control Center modes and editor curves.
//!
//! Firmware modes send a WMI script. Curve presets are custom tables. Where
//! the factory points were not published, the curve is a safe stand-in and
//! `provisional` is set. Those points are not a dump of Control Center.

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
            provisional: true,
            notes: "Same firmware switch. The exact flag combination is provisional; see the quiet script.",
            kind: PresetKind::Firmware(FanMode::Quiet),
        },
        Preset {
            id: "gaming",
            name: "Gaming",
            provisional: true,
            notes: "Firmware gaming profile. Fan writes currently match Normal; see the gaming script.",
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
        curve_preset(
            "deep-control",
            "Deep Control",
            true,
            "PROVISIONAL. Control Center's manual describes a 15-point Deep Control editor and does not publish the factory points. The 46°C / 44% sentence in that manual is an example of a broken rule, not a default. This stand-in is the Balanced curve.",
            &BALANCED,
        ),
        curve_preset(
            "silent",
            "Silent",
            true,
            "PROVISIONAL editor curve. It still obeys the 85°C and last-point floors, so it will not stay quiet once the CPU is hot.",
            &SILENT,
        ),
        curve_preset(
            "balanced",
            "Balanced",
            true,
            "PROVISIONAL editor curve. Not a Control Center dump.",
            &BALANCED,
        ),
        curve_preset(
            "performance",
            "Performance",
            true,
            "PROVISIONAL editor curve shaped like a gaming table. The Gaming button does not upload this; Upload curve does.",
            &PERFORMANCE,
        ),
        curve_preset(
            "max-ish",
            "Max-ish",
            true,
            "PROVISIONAL aggressive custom table. Firmware Max is the Max button, which sends raw 229 rather than this curve.",
            &MAXISH,
        ),
    ]
}

const SILENT: &[(u8, u8)] = &[(55, 35), (70, 40), (80, 48), (85, 55), (90, 80)];
const BALANCED: &[(u8, u8)] = &[(45, 35), (60, 45), (75, 60), (85, 80), (90, 100)];
const PERFORMANCE: &[(u8, u8)] = &[(40, 40), (55, 55), (70, 75), (80, 90), (90, 100)];
const MAXISH: &[(u8, u8)] = &[(35, 55), (50, 70), (65, 85), (75, 95), (90, 100)];

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
                name: preset.name.to_string(),
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
                expert_mode: false,
                provisional: preset.provisional,
                notes: preset.notes.to_string(),
                cpu_pl1_w: None,
                cpu_pl2_w: None,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_curve_validates_and_unconfirmed_ones_are_marked() {
        let presets = presets();
        assert!(presets
            .iter()
            .any(|preset| preset.id == "deep-control" && preset.provisional));
        assert!(presets
            .iter()
            .any(|preset| preset.id == "quiet" && preset.provisional));
        assert!(presets
            .iter()
            .any(|preset| preset.id == "normal" && !preset.provisional));
        for preset in &presets {
            if let PresetKind::Curve(curve) = &preset.kind {
                assert!(preset.provisional, "{}", preset.id);
                assert!(curve
                    .points()
                    .iter()
                    .all(|point| !(point.temp_c == 0 && point.duty_raw == 0)));
            }
        }
    }

    #[test]
    fn builtin_config_round_trips_through_toml() {
        let config = builtin_config();
        let parsed = Config::from_toml(&config.to_toml().unwrap()).unwrap();
        assert_eq!(parsed.ac_profile.as_deref(), Some("Gaming"));
        assert_eq!(parsed.dc_profile.as_deref(), Some("Quiet"));
        let deep = parsed.profile("Deep Control").unwrap();
        assert_eq!(deep.fan_mode, FanMode::Custom);
        assert!(deep.provisional);
        let curve = deep.fan_curve().unwrap();
        assert_eq!(curve.points().len(), BALANCED.len());
        assert_eq!(curve.points()[0].duty_raw, percent_to_raw(35));
    }
}
