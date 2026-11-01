//! Per-user profiles. Loading never creates or rewrites a settings file.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use aero_core::{Config, FanMode, Profile};

const MAX_FILE_BYTES: u64 = 1024 * 1024;
static SAVE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedProfiles {
    pub config: Config,
    pub apply_at_startup: bool,
    pub switch_on_power_change: bool,
    pub reapply_on_resume: bool,
    pub keep_in_tray: bool,
    pub start_minimized: bool,
    pub fan_hotkey: bool,
    pub wifi_hotkey: bool,
    pub display_off_hotkey: bool,
    pub display: crate::display_controls::DisplaySettings,
}

impl Default for SavedProfiles {
    fn default() -> Self {
        Self {
            config: aero_core::builtin_config(),
            apply_at_startup: false,
            switch_on_power_change: false,
            reapply_on_resume: false,
            keep_in_tray: false,
            start_minimized: false,
            fan_hotkey: false,
            wifi_hotkey: false,
            display_off_hotkey: false,
            display: Default::default(),
        }
    }
}

pub fn default_path() -> Result<PathBuf, String> {
    std::env::var_os("APPDATA")
        .map(|root| {
            PathBuf::from(root)
                .join("aero-control")
                .join("profiles.toml")
        })
        .ok_or_else(|| "The user settings directory is unavailable (APPDATA is unset).".into())
}

pub fn load(path: &Path) -> Result<SavedProfiles, String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SavedProfiles::default());
        }
        Err(error) => return Err(format!("Could not open {}: {error}", path.display())),
    };
    let mut text = String::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err("The profiles file exceeds the 1 MiB limit.".into());
    }
    parse(&text).map_err(|error| format!("Invalid profiles file {}: {error}", path.display()))
}

pub fn parse(text: &str) -> Result<SavedProfiles, String> {
    let root: toml::Table = toml::from_str(text).map_err(|error| error.to_string())?;
    let saved = SavedProfiles {
        config: Config::from_toml(text).map_err(|error| error.to_string())?,
        apply_at_startup: top_level_flag(&root, "apply_at_startup")?,
        switch_on_power_change: top_level_flag(&root, "switch_on_power_change")?,
        reapply_on_resume: top_level_flag(&root, "reapply_on_resume")?,
        keep_in_tray: top_level_flag(&root, "keep_in_tray")?,
        start_minimized: top_level_flag(&root, "start_minimized")?,
        fan_hotkey: top_level_flag(&root, "fan_hotkey")?,
        wifi_hotkey: top_level_flag(&root, "wifi_hotkey")?,
        display_off_hotkey: top_level_flag(&root, "display_off_hotkey")?,
        display: crate::display_controls::DisplaySettings::from_table(&root)?,
    };
    validate(&saved)?;
    Ok(saved)
}

fn top_level_flag(root: &toml::Table, key: &str) -> Result<bool, String> {
    match root.get(key) {
        None => Ok(false),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| format!("{key} must be true or false")),
    }
}

pub fn validate(saved: &SavedProfiles) -> Result<(), String> {
    if saved.start_minimized && !saved.keep_in_tray {
        return Err(
            "Starting in the notification area requires keeping the app in the notification area."
                .into(),
        );
    }
    saved.display.validate()?;
    if saved.config.profiles.is_empty() || saved.config.profiles.len() > 256 {
        return Err("A profiles file must contain 1 to 256 profiles.".into());
    }
    let mut names = HashSet::new();
    for profile in &saved.config.profiles {
        validate_profile(profile)?;
        if !names.insert(profile.name.to_lowercase()) {
            return Err(format!("Duplicate profile name: {}", profile.name));
        }
    }
    for name in [
        Some(saved.config.active.as_str()),
        saved.config.ac_profile.as_deref(),
        saved.config.dc_profile.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if saved.config.profile(name).is_none() {
            return Err(format!("The selected profile {name:?} does not exist."));
        }
    }
    Ok(())
}

pub fn validate_profile(profile: &Profile) -> Result<(), String> {
    if profile.cpu_power_preset.is_some()
        && (profile.cpu_pl1_w.is_some() || profile.cpu_pl2_w.is_some())
    {
        return Err("Use a CPU power preset or legacy CPU fields, not both.".into());
    }
    if profile.name.trim() != profile.name
        || profile.name.is_empty()
        || profile.name.chars().count() > 64
        || profile.name.chars().any(char::is_control)
    {
        return Err("Use a profile name of 1 to 64 characters without leading/trailing spaces or control characters.".into());
    }
    if profile.fixed_duty_percent.is_some_and(|value| value > 100)
        || profile.fixed_duty_raw.is_some_and(|value| value > 229)
    {
        return Err(format!("{} has an invalid fixed duty.", profile.name));
    }
    if profile
        .charge_limit
        .is_some_and(|limit| !(60..=100).contains(&limit))
    {
        return Err(format!(
            "{} has a charge limit outside 60–100%.",
            profile.name
        ));
    }
    if profile.nv_power_config.is_some_and(|value| value > 1)
        || profile.nv_thermal_target.is_some_and(|value| value > 1)
    {
        return Err(format!(
            "{} has a GPU switch value other than 0 or 1.",
            profile.name
        ));
    }
    if profile
        .charge_policy
        .is_some_and(|policy| policy != 0 && policy != 4)
        || (profile.charge_policy == Some(4) && profile.charge_limit.is_none())
        || (profile.charge_policy == Some(0) && profile.charge_limit.is_some())
    {
        return Err(format!(
            "{} has an inconsistent charging policy.",
            profile.name
        ));
    }
    if profile
        .curve
        .iter()
        .any(|point| point.duty_percent.is_some_and(|value| value > 100))
        || profile.curve_pairs.iter().any(|point| point[1] > 100)
    {
        return Err(format!(
            "{} has a curve percentage above 100%.",
            profile.name
        ));
    }
    if profile.fan_mode == FanMode::Custom {
        profile.fan_curve().map_err(|issues| {
            format!(
                "{}: {}",
                profile.name,
                issues
                    .iter()
                    .map(|issue| issue.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        })?;
    }
    Ok(())
}

pub fn save(path: &Path, saved: &SavedProfiles) -> Result<(), String> {
    validate(saved)?;
    let mut root: toml::Table =
        toml::from_str(&saved.config.to_toml().map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    for (name, enabled) in [
        ("apply_at_startup", saved.apply_at_startup),
        ("switch_on_power_change", saved.switch_on_power_change),
        ("reapply_on_resume", saved.reapply_on_resume),
        ("keep_in_tray", saved.keep_in_tray),
        ("start_minimized", saved.start_minimized),
        ("fan_hotkey", saved.fan_hotkey),
        ("wifi_hotkey", saved.wifi_hotkey),
        ("display_off_hotkey", saved.display_off_hotkey),
    ] {
        root.insert(name.into(), enabled.into());
    }
    saved.display.write_table(&mut root);
    let body = format!(
        "# aero-control profiles\n{}",
        toml::to_string_pretty(&root).map_err(|error| error.to_string())?
    );
    if body.len() as u64 > MAX_FILE_BYTES {
        return Err("The profiles file exceeds the 1 MiB limit.".into());
    }
    let parent = path
        .parent()
        .ok_or("The profiles path has no parent directory.")?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    let suffix = SAVE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".profiles-{}-{suffix}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("Could not create {}: {error}", temporary.display()))?;
    let result = (|| {
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if let Err(error) = result {
        // Remove only this operation's temporary file; the destination is intact.
        let _ = fs::remove_file(&temporary);
        return Err(format!("Could not save {}: {error}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_preset_roundtrips_and_conflicting_legacy_limits_are_rejected() {
        let mut saved = SavedProfiles::default();
        saved.config.profiles[0].cpu_power_preset = Some(aero_core::CpuPowerPreset::Battery);
        let encoded = saved.config.to_toml().unwrap();
        assert_eq!(parse(&encoded).unwrap().config, saved.config);
        assert!(encoded.contains("cpu_power_preset = \"battery\""));
        saved.config.profiles[0].cpu_power_preset = Some(aero_core::CpuPowerPreset::Level1);
        let encoded = saved.config.to_toml().unwrap();
        assert!(encoded.contains("cpu_power_preset = \"level-1\""));
        assert_eq!(parse(&encoded).unwrap().config, saved.config);
        saved.config.profiles[0].cpu_pl1_w = Some(45);
        assert!(validate(&saved).is_err());
    }

    fn test_path() -> PathBuf {
        // Keep filesystem tests inside the build workspace; some Windows
        // application sandboxes allow writing TEMP files but deny renaming them.
        std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!(
                "aero-profiles-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
            .join("profiles.toml")
    }

    #[test]
    fn roundtrip_keeps_factory_raw_values_and_opt_in_automation() {
        let path = test_path();
        let mut original = SavedProfiles::default();
        assert!(
            !original.apply_at_startup
                && !original.switch_on_power_change
                && !original.reapply_on_resume
        );
        original.apply_at_startup = true;
        original.switch_on_power_change = true;
        original.reapply_on_resume = true;
        original.display = crate::display_controls::DisplaySettings {
            automatic: true,
            display_id: Some("mock-internal-display".into()),
            ac_hz: Some(144),
            dc_hz: Some(60),
        };
        original.wifi_hotkey = true;
        original.display_off_hotkey = true;
        original.keep_in_tray = true;
        original.start_minimized = true;
        save(&path, &original).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded, original);
        assert_eq!(
            loaded
                .config
                .profile("Deep Control")
                .unwrap()
                .fan_curve()
                .unwrap()
                .points()[1]
                .duty_raw,
            68
        );
        fs::remove_file(&path).unwrap();
        fs::remove_dir(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn invalid_new_settings_do_not_overwrite_the_existing_file() {
        let path = test_path();
        let original = SavedProfiles::default();
        save(&path, &original).unwrap();
        let bytes = fs::read(&path).unwrap();
        let mut invalid = original;
        invalid.config.ac_profile = Some("Missing".into());
        assert!(save(&path, &invalid).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::remove_file(&path).unwrap();
        fs::remove_dir(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn invalid_automation_values_fail_closed() {
        let text = SavedProfiles::default().config.to_toml().unwrap();
        assert!(parse(&format!("apply_at_startup = \"true\"\n{text}")).is_err());
        assert!(!parse(&text).unwrap().apply_at_startup);
        assert!(!parse(&text).unwrap().wifi_hotkey);
        assert!(parse(&format!("wifi_hotkey = \"true\"\n{text}")).is_err());
    }

    #[test]
    fn text_inside_multiline_strings_cannot_enable_automation() {
        let config = SavedProfiles::default().config.to_toml().unwrap();
        let text = format!("notes = '''\napply_at_startup = true\nswitch_on_power_change = true\nreapply_on_resume = true\n'''\n{config}");
        let parsed = parse(&text).unwrap();
        assert!(!parsed.apply_at_startup);
        assert!(!parsed.switch_on_power_change);
        assert!(!parsed.reapply_on_resume);
    }

    #[test]
    fn duplicate_automation_keys_are_rejected() {
        let config = SavedProfiles::default().config.to_toml().unwrap();
        assert!(parse(&format!(
            "apply_at_startup = false\napply_at_startup = true\n{config}"
        ))
        .is_err());
    }

    #[test]
    fn replacing_a_saved_file_is_supported() {
        let path = test_path();
        let mut saved = SavedProfiles::default();
        save(&path, &saved).unwrap();
        saved.config.active = "Gaming".into();
        save(&path, &saved).unwrap();
        assert_eq!(load(&path).unwrap().config.active, "Gaming");
        fs::remove_file(&path).unwrap();
        fs::remove_dir(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn minimized_start_requires_tray_and_new_flags_are_explicit_booleans() {
        let config = SavedProfiles::default().config.to_toml().unwrap();
        let defaults = parse(&config).unwrap();
        assert!(!defaults.start_minimized);
        assert!(!defaults.display_off_hotkey);
        assert!(parse(&format!("start_minimized = true\n{config}")).is_err());
        let enabled = parse(&format!(
            "keep_in_tray = true\nstart_minimized = true\ndisplay_off_hotkey = true\n{config}"
        ))
        .unwrap();
        assert!(enabled.start_minimized && enabled.keep_in_tray && enabled.display_off_hotkey);
        assert!(parse(&format!("display_off_hotkey = \"true\"\n{config}")).is_err());
        assert!(parse(&format!("start_minimized = \"true\"\n{config}")).is_err());
    }

    #[test]
    fn legacy_software_settings_load_without_starting_or_retaining_a_controller() {
        let path = test_path();
        let config = SavedProfiles::default().config.to_toml().unwrap();
        let legacy = format!("{config}\n[software_fan]\ninterval_ms = 1500\nramp_up_percent_per_second = 20\nramp_down_percent_per_second = 5\n");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &legacy).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), legacy);
        save(&path, &loaded).unwrap();
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("[software_fan]"));
        assert_eq!(load(&path).unwrap(), loaded);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(path.parent().unwrap()).unwrap();
    }
}
