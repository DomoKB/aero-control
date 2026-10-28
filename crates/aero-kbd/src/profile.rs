//! Control Center's two kinds of saved lighting state, kept in a plain text file
//! owned by aero-control (Control Center's own registry keys and ProfileData.xml
//! are never written):
//!  * per-effect memory, like `HKLM\Software\GigabyteRGBFusion\<effect>`
//!    = "brightness|speed|color|direction|oldcolor";
//!  * numbered profiles, like ProfileData.xml (LightEffect, Color, Brightness, Speed, Direction).

use std::collections::HashMap;

use crate::proto::{Color, Direction, Effect, LightSettings};

pub const PROFILE_COUNT: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub settings: LightSettings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    /// Last values used for each effect (Control Center restores these on switch).
    pub per_effect: HashMap<&'static str, (LightSettings, Color)>,
    pub profiles: Vec<Profile>,
    pub current_profile: usize,
}

impl Default for Store {
    fn default() -> Self {
        Store {
            per_effect: HashMap::new(),
            profiles: default_profiles(),
            current_profile: 0,
        }
    }
}

/// Control Center's shipped ProfileData.xml: Profile 1 Static Orange speed 7,
/// Profile 2 Static Orange (its stored speed 50 is outside 1..=10 and is clamped to 10),
/// Profiles 3-5 Static Blue speed 3; all brightness 50, direction 1.
pub fn default_profiles() -> Vec<Profile> {
    let mk = |n: usize, color, speed| Profile {
        name: format!("PROFILE {n:02}"),
        settings: LightSettings {
            effect: Effect::Static,
            color,
            brightness: 50,
            speed,
            direction: Direction::LeftToRight,
        },
    };
    vec![
        mk(1, Color::Orange, 7),
        mk(2, Color::Orange, 10),
        mk(3, Color::Blue, 3),
        mk(4, Color::Blue, 3),
        mk(5, Color::Blue, 3),
    ]
}

impl Store {
    /// Settings to show when switching to `effect`: saved ones, else Control Center's default.
    pub fn recall(&self, effect: Effect) -> (LightSettings, Color) {
        self.per_effect
            .get(effect.key())
            .copied()
            .unwrap_or((LightSettings::effect_default(effect), Color::Orange))
    }

    pub fn remember(&mut self, settings: LightSettings, old_color: Color) {
        self.per_effect
            .insert(settings.effect.key(), (settings, old_color));
    }

    pub fn serialize(&self) -> String {
        let mut out = String::from("# aero-control keyboard lighting\n");
        out.push_str(&format!("current_profile={}\n", self.current_profile + 1));
        let mut keys: Vec<_> = self.per_effect.keys().copied().collect();
        keys.sort_unstable();
        for key in keys {
            let (s, old) = self.per_effect[key];
            out.push_str(&format!(
                "effect.{key}={}|{}|{}|{}|{}\n",
                s.brightness,
                s.speed,
                s.color.code(),
                s.direction.code(),
                old.code()
            ));
        }
        for (i, p) in self.profiles.iter().enumerate() {
            let s = p.settings;
            out.push_str(&format!(
                "profile.{}={}|{}|{}|{}|{}|{}\n",
                i + 1,
                s.effect.key(),
                s.color.code(),
                s.brightness,
                s.speed,
                s.direction.code(),
                p.name.replace(['|', '\n'], " ")
            ));
        }
        out
    }

    /// Lenient parse: bad lines are skipped and every value is re-validated.
    pub fn parse(text: &str) -> Store {
        let mut store = Store::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let f: Vec<&str> = v.split('|').collect();
            let num = |i: usize| f.get(i).and_then(|x| x.trim().parse::<u8>().ok());
            if k == "current_profile" {
                if let Some(n) = v
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .filter(|n| (1..=PROFILE_COUNT).contains(n))
                {
                    store.current_profile = n - 1;
                }
            } else if let Some(key) = k.strip_prefix("effect.") {
                let Some(effect) = Effect::from_key(key) else {
                    continue;
                };
                let (Some(b), Some(sp), Some(c), Some(d), Some(o)) =
                    (num(0), num(1), num(2), num(3), num(4))
                else {
                    continue;
                };
                let (Some(color), Some(direction), Some(old)) = (
                    Color::from_code(c),
                    Direction::from_code(d),
                    Color::from_code(o),
                ) else {
                    continue;
                };
                let s = LightSettings {
                    effect,
                    color,
                    brightness: b,
                    speed: sp,
                    direction,
                }
                .normalized();
                if s.validate().is_ok() {
                    store.remember(s, old);
                }
            } else if let Some(idx) = k.strip_prefix("profile.") {
                let Some(i) = idx
                    .parse::<usize>()
                    .ok()
                    .filter(|i| (1..=PROFILE_COUNT).contains(i))
                else {
                    continue;
                };
                let Some(effect) = f.first().and_then(|e| Effect::from_key(e)) else {
                    continue;
                };
                let (Some(c), Some(b), Some(sp), Some(d)) = (num(1), num(2), num(3), num(4)) else {
                    continue;
                };
                let (Some(color), Some(direction)) = (Color::from_code(c), Direction::from_code(d))
                else {
                    continue;
                };
                let s = LightSettings {
                    effect,
                    color,
                    brightness: b,
                    speed: sp,
                    direction,
                }
                .normalized();
                if s.validate().is_ok() {
                    let name = f
                        .get(5)
                        .map(|n| n.trim().to_string())
                        .filter(|n| !n.is_empty());
                    store.profiles[i - 1] = Profile {
                        name: name.unwrap_or_else(|| format!("PROFILE {i:02}")),
                        settings: s,
                    };
                }
            }
        }
        store
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut st = Store {
            current_profile: 2,
            ..Default::default()
        };
        st.remember(LightSettings::effect_default(Effect::Ripple), Color::Green);
        st.profiles[4].settings = LightSettings::effect_default(Effect::Wave);
        st.profiles[4].name = "Night".into();
        assert_eq!(Store::parse(&st.serialize()), st);
    }

    #[test]
    fn recall_defaults() {
        let st = Store::default();
        let (s, old) = st.recall(Effect::Hedge);
        assert_eq!((s.brightness, s.speed, s.color), (50, 5, Color::Red));
        assert_eq!(old, Color::Orange);
    }

    #[test]
    fn rejects_out_of_range() {
        let st = Store::parse("effect.static=99|5|1|1|5\nprofile.1=static|1|50|0|1|x\n");
        assert!(st.per_effect.is_empty());
        assert_eq!(st.profiles, default_profiles());
    }
}
