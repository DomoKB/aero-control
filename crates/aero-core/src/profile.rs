use serde::{Deserialize, Serialize};

use crate::curve::{CurveIssue, CurvePoint, FanCurve};
use crate::mode::FanMode;
use crate::scale::percent_to_raw;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurvePointToml {
    pub temp_c: u8,
    #[serde(default)]
    pub duty_percent: Option<u8>,
    /// When set, this exact raw duty is what gets uploaded. Percent is display.
    #[serde(default)]
    pub duty_raw: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub fan_mode: FanMode,
    #[serde(default)]
    pub fixed_duty_percent: Option<u8>,
    #[serde(default)]
    pub fixed_duty_raw: Option<u8>,
    #[serde(default)]
    pub curve: Vec<CurvePointToml>,
    /// `[temperature, duty percent]` pairs. Used when `curve` is empty.
    #[serde(default)]
    pub curve_pairs: Vec<[u8; 2]>,
    #[serde(default)]
    pub dynamic_boost: Option<bool>,
    #[serde(default)]
    pub nv_power_config: Option<u8>,
    #[serde(default)]
    pub nv_thermal_target: Option<u8>,
    #[serde(default)]
    pub ai_boost: Option<u8>,
    #[serde(default)]
    pub charge_limit: Option<u8>,
    #[serde(default)]
    pub expert_mode: bool,
    #[serde(default)]
    pub provisional: bool,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub cpu_pl1_w: Option<u16>,
    #[serde(default)]
    pub cpu_pl2_w: Option<u16>,
}

impl Profile {
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    pub fn curve_points(&self) -> Vec<CurvePoint> {
        if !self.curve.is_empty() {
            self.curve
                .iter()
                .map(|entry| CurvePoint {
                    temp_c: entry.temp_c,
                    duty_raw: entry
                        .duty_raw
                        .unwrap_or_else(|| percent_to_raw(entry.duty_percent.unwrap_or(0))),
                })
                .collect()
        } else {
            self.curve_pairs
                .iter()
                .map(|[temp, percent]| CurvePoint {
                    temp_c: *temp,
                    duty_raw: percent_to_raw(*percent),
                })
                .collect()
        }
    }

    pub fn fan_curve(&self) -> Result<FanCurve, Vec<CurveIssue>> {
        FanCurve::try_new(self.curve_points())
    }

    pub fn has_deferred_fields(&self) -> bool {
        self.dynamic_boost.is_some()
            || self.nv_power_config.is_some()
            || self.nv_thermal_target.is_some()
            || self.ai_boost.is_some()
            || self.charge_limit.is_some()
            || self.cpu_pl1_w.is_some()
            || self.cpu_pl2_w.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub active: String,
    #[serde(default)]
    pub ac_profile: Option<String>,
    #[serde(default)]
    pub dc_profile: Option<String>,
    pub profiles: Vec<Profile>,
}

impl Config {
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|profile| profile.name == name)
    }

    /// AC/DC binding, falling back to `active` when that side is unset.
    pub fn profile_for_power(&self, on_ac: bool) -> Result<&Profile, String> {
        let bound = if on_ac {
            self.ac_profile.as_deref()
        } else {
            self.dc_profile.as_deref()
        };
        let name = bound.unwrap_or(self.active.as_str());
        self.profile(name)
            .ok_or_else(|| format!("profile \"{name}\" is not in the config"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scale::percent_to_raw;

    #[test]
    fn toml_round_trip_keeps_raw_duty_and_mode_aliases() {
        let text = r#"
            name = "Evening"
            fan_mode = "super-quiet"
            fixed_duty_percent = 40
            charge_limit = 80
            notes = "alias"
            curve_pairs = [[45, 35], [85, 80], [90, 100]]
        "#;
        let profile = Profile::from_toml(text).unwrap();
        assert_eq!(profile.fan_mode, FanMode::Quiet);
        let curve = profile.fan_curve().unwrap();
        assert_eq!(curve.points()[0].duty_raw, percent_to_raw(35));

        let stored = r#"
            name = "Exact"
            fan_mode = "custom"
            [[curve]]
            temp_c = 85
            duty_percent = 50
            duty_raw = 114
            [[curve]]
            temp_c = 90
            duty_percent = 80
            duty_raw = 183
        "#;
        let profile = Profile::from_toml(stored).unwrap();
        let curve = profile.fan_curve().unwrap();
        assert_eq!(curve.points()[0].duty_raw, 114);
    }

    #[test]
    fn power_source_selects_the_bound_profile() {
        let config = Config::from_toml(
            r#"
            active = "Normal"
            ac_profile = "Gaming"
            dc_profile = "Quiet"
            [[profiles]]
            name = "Normal"
            fan_mode = "normal"
            [[profiles]]
            name = "Gaming"
            fan_mode = "gaming"
            [[profiles]]
            name = "Quiet"
            fan_mode = "quiet"
            "#,
        )
        .unwrap();
        assert_eq!(
            config.profile_for_power(true).unwrap().fan_mode,
            FanMode::Gaming
        );
        assert_eq!(
            config.profile_for_power(false).unwrap().fan_mode,
            FanMode::Quiet
        );
    }

    #[test]
    fn shipped_profile_files_load_and_custom_curves_validate() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles");
        let config =
            Config::from_toml(&std::fs::read_to_string(root.join("config.toml")).unwrap()).unwrap();
        assert!(config.profile("Deep Control").unwrap().fan_curve().is_ok());
        assert!(config.profile("Deep Control").unwrap().provisional);
        for name in [
            "silent.toml",
            "balanced.toml",
            "performance.toml",
            "max-ish.toml",
            "pairs.toml",
        ] {
            let profile =
                Profile::from_toml(&std::fs::read_to_string(root.join(name)).unwrap()).unwrap();
            assert!(profile.fan_curve().is_ok(), "{name}");
            assert!(profile.provisional, "{name}");
        }
    }
}
