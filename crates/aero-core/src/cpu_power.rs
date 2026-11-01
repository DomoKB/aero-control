//! Official AERO 17 YB i7 power presets (CPUOC.Resource.P77VB.xml).
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CpuPowerPreset {
    #[serde(rename = "level-1")]
    Level1,
    #[serde(rename = "level-2")]
    Level2,
    #[serde(rename = "level-3")]
    Level3,
    #[serde(rename = "level-4")]
    Level4,
    #[serde(rename = "level-5")]
    Level5,
    Default,
    Battery,
}

impl CpuPowerPreset {
    pub const ALL: [Self; 7] = [
        Self::Level1,
        Self::Level2,
        Self::Level3,
        Self::Level4,
        Self::Level5,
        Self::Default,
        Self::Battery,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Level1 => "Level 1 · 38 W",
            Self::Level2 => "Level 2 · 45 W",
            Self::Level3 => "Level 3 · 52 W",
            Self::Level4 => "Level 4 · 58 W",
            Self::Level5 => "Level 5 · 62 W",
            Self::Default => "Official default · 52 W",
            Self::Battery => "Official battery preset · 45 W",
        }
    }

    /// Milliwatts keep the official battery burst limit of 56.25 W exact.
    pub fn limits_mw(self) -> (u32, u32) {
        match self {
            Self::Level1 => (38_000, 107_000),
            Self::Level2 => (45_000, 107_000),
            Self::Level3 | Self::Default => (52_000, 107_000),
            Self::Level4 => (58_000, 107_000),
            Self::Level5 => (62_000, 107_000),
            Self::Battery => (45_000, 56_250),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_keep_the_official_yb_i7_limits_and_fractional_battery_limit() {
        assert_eq!(
            CpuPowerPreset::ALL.map(CpuPowerPreset::limits_mw),
            [
                (38_000, 107_000),
                (45_000, 107_000),
                (52_000, 107_000),
                (58_000, 107_000),
                (62_000, 107_000),
                (52_000, 107_000),
                (45_000, 56_250)
            ]
        );
    }
}
