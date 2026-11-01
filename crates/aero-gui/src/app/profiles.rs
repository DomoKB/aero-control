//! Saved fan profiles and opt-in power-source automation.

use std::path::PathBuf;

use aero_core::{FanMode, Profile};
use eframe::egui::{self, RichText};

use super::{
    banner, card, page_header, primary_button, secondary_button, section_title, DANGER, MUTED, TEXT,
};
use crate::profile_store::{self, SavedProfiles};

pub enum ProfileAction {
    Apply(Profile),
    /// Parent supplies a snapshot of its confirmed fan settings with this name.
    Capture(String),
    /// Settings were saved successfully; parent should update its automation policy.
    AutomationChanged,
}

pub struct ProfilesTab {
    saved: SavedProfiles,
    path: Option<PathBuf>,
    load_error: Option<String>,
    selected: String,
    new_name: String,
    startup_draft: bool,
    switch_draft: bool,
    resume_draft: bool,
    tray_draft: bool,
    minimized_draft: bool,
    hotkey_draft: bool,
    wifi_hotkey_draft: bool,
    display_off_hotkey_draft: bool,
    ac_draft: Option<String>,
    dc_draft: Option<String>,
    note: Option<(String, bool)>,
}

impl ProfilesTab {
    pub fn new(mock: bool) -> Self {
        let (path, result) = if mock {
            (None, Ok(SavedProfiles::default()))
        } else {
            match profile_store::default_path() {
                Ok(path) => {
                    let result = profile_store::load(&path);
                    (Some(path), result)
                }
                Err(error) => (None, Err(error)),
            }
        };
        let (saved, load_error) = match result {
            Ok(saved) => (saved, None),
            Err(error) => (SavedProfiles::default(), Some(error)),
        };
        Self {
            selected: saved.config.active.clone(),
            new_name: String::new(),
            startup_draft: saved.apply_at_startup,
            switch_draft: saved.switch_on_power_change,
            resume_draft: saved.reapply_on_resume,
            tray_draft: saved.keep_in_tray,
            minimized_draft: saved.start_minimized,
            hotkey_draft: saved.fan_hotkey,
            wifi_hotkey_draft: saved.wifi_hotkey,
            display_off_hotkey_draft: saved.display_off_hotkey,
            ac_draft: saved.config.ac_profile.clone(),
            dc_draft: saved.config.dc_profile.clone(),
            saved,
            path,
            load_error,
            note: None,
        }
    }

    pub fn settings(&self) -> &SavedProfiles {
        &self.saved
    }

    pub fn save_display_settings(
        &mut self,
        display: crate::display_controls::DisplaySettings,
    ) -> Result<(), String> {
        let mut next = self.saved.clone();
        next.display = display;
        self.commit(next)
    }

    /// Invalid files never enable automation through the default fallback.
    pub fn automation_profile(&self, on_ac: bool) -> Option<Profile> {
        if self.load_error.is_some() || !self.saved.switch_on_power_change {
            return None;
        }
        self.saved.config.profile_for_power(on_ac).ok().cloned()
    }

    pub fn startup_profile(&self, on_ac: Option<bool>) -> Option<Profile> {
        if self.load_error.is_some() || !self.saved.apply_at_startup {
            return None;
        }
        if self.saved.switch_on_power_change {
            if let Some(on_ac) = on_ac {
                return self.saved.config.profile_for_power(on_ac).ok().cloned();
            }
        }
        self.saved
            .config
            .profile(&self.saved.config.active)
            .cloned()
    }

    pub fn resume_profile(&self, on_ac: Option<bool>) -> Option<Profile> {
        if self.load_error.is_some() || !self.saved.reapply_on_resume {
            return None;
        }
        if self.saved.switch_on_power_change {
            if let Some(on_ac) = on_ac {
                return self.saved.config.profile_for_power(on_ac).ok().cloned();
            }
        }
        self.saved
            .config
            .profile(&self.saved.config.active)
            .cloned()
    }

    fn commit(&mut self, next: SavedProfiles) -> Result<(), String> {
        if self.load_error.is_some() {
            return Err("Settings were not saved because the existing file could not be loaded. Correct that file and reload it first.".into());
        }
        profile_store::validate(&next)?;
        if let Some(path) = &self.path {
            profile_store::save(path, &next)?;
        }
        self.saved = next;
        Ok(())
    }

    /// Saves an exact snapshot. Built-in profiles cannot be replaced.
    pub fn capture(&mut self, mut profile: Profile) -> Result<(), String> {
        profile.name = profile.name.trim().to_string();
        let result: Result<(), String> = (|| {
            profile_store::validate_profile(&profile)?;
            if is_builtin(&profile.name) {
                return Err("Choose a new name; built-in profiles are kept unchanged.".into());
            }
            let mut next = self.saved.clone();
            if let Some(existing) = next
                .config
                .profiles
                .iter_mut()
                .find(|p| p.name == profile.name)
            {
                *existing = profile.clone();
            } else {
                next.config.profiles.push(profile.clone());
            }
            self.commit(next)?;
            self.selected = profile.name.clone();
            Ok(())
        })();
        self.note = Some(match &result {
            Ok(()) => (
                format!("Saved {}. Saving does not apply it.", profile.name),
                false,
            ),
            Err(error) => (error.clone(), true),
        });
        result
    }

    /// Call only after a profile apply succeeds, not after the request is queued.
    pub fn mark_applied(&mut self, name: &str) -> Result<(), String> {
        let mut next = self.saved.clone();
        next.config.active = name.to_string();
        self.commit(next)
    }

    fn delete_selected(&mut self) -> Result<(), String> {
        if is_builtin(&self.selected) {
            return Err("Built-in profiles cannot be deleted.".into());
        }
        let mut next = self.saved.clone();
        next.config
            .profiles
            .retain(|profile| profile.name != self.selected);
        if next.config.active == self.selected {
            next.config.active = "Normal".into();
        }
        if next.config.ac_profile.as_deref() == Some(&self.selected) {
            next.config.ac_profile = None;
        }
        if next.config.dc_profile.as_deref() == Some(&self.selected) {
            next.config.dc_profile = None;
        }
        self.commit(next)?;
        self.selected = self.saved.config.active.clone();
        self.ac_draft = self.saved.config.ac_profile.clone();
        self.dc_draft = self.saved.config.dc_profile.clone();
        self.note = Some((
            "Saved profile removed. The current fan mode has not changed.".into(),
            false,
        ));
        Ok(())
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        can_apply: bool,
        can_capture: bool,
    ) -> Vec<ProfileAction> {
        let mut actions = Vec::new();
        page_header(
            ui,
            "Profiles",
            "Save exact fan settings and choose what happens on charger or battery.",
        );
        if let Some(error) = &self.load_error {
            banner(ui, error, DANGER);
            ui.label("The existing file is preserved. Saving and automation are disabled until it loads successfully.");
            if ui.button("Reload profiles file").clicked() {
                if let Some(path) = &self.path {
                    match profile_store::load(path) {
                        Ok(saved) => {
                            self.selected = saved.config.active.clone();
                            self.startup_draft = saved.apply_at_startup;
                            self.switch_draft = saved.switch_on_power_change;
                            self.resume_draft = saved.reapply_on_resume;
                            self.tray_draft = saved.keep_in_tray;
                            self.hotkey_draft = saved.fan_hotkey;
                            self.ac_draft = saved.config.ac_profile.clone();
                            self.dc_draft = saved.config.dc_profile.clone();
                            self.saved = saved;
                            self.load_error = None;
                            actions.push(ProfileAction::AutomationChanged);
                        }
                        Err(error) => self.load_error = Some(error),
                    }
                }
            }
            ui.add_space(10.0);
        }
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, "Saved profiles");
            ui.add_space(8.0);
            egui::ComboBox::from_id_salt("saved_profile")
                .selected_text(&self.selected)
                .width(230.0)
                .show_ui(ui, |ui| {
                    for profile in &self.saved.config.profiles {
                        ui.selectable_value(&mut self.selected, profile.name.clone(), &profile.name);
                    }
                });
            if let Some(profile) = self.saved.config.profile(&self.selected).cloned() {
                let description = match profile.fan_mode {
                    FanMode::Custom => format!("Custom curve · {} points", profile.curve_points().len()),
                    FanMode::Fixed => format!("Fixed · {}%", profile.fixed_duty_raw.map(aero_core::raw_to_percent).or(profile.fixed_duty_percent).unwrap_or(50)),
                    mode => format!("{mode:?} mode"),
                };
                ui.label(RichText::new(description).color(TEXT));
                if let Some(preset) = profile.cpu_power_preset {
                    let (pl1, pl2) = preset.limits_mw();
                    ui.label(RichText::new(format!("CPU: {:.2} W sustained · {:.2} W short burst", pl1 as f64 / 1000.0, pl2 as f64 / 1000.0)).color(MUTED));
                }
                ui.label(RichText::new(match profile.charge_policy {
                    Some(0) => "Charging: Standard".to_string(),
                    _ => profile.charge_limit.map_or("Charging: leave unchanged".to_string(), |limit| format!("Charging: stop at {limit}%")),
                }).color(MUTED));
                if !profile.notes.is_empty() {
                    ui.label(RichText::new(&profile.notes).color(MUTED));
                }
                if profile.has_deferred_fields() {
                    ui.label("This profile includes advanced fields. The apply result will identify settings that could not be applied.");
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(can_apply, primary_button("Apply profile")).clicked() {
                        actions.push(ProfileAction::Apply(profile));
                    }
                    if ui.add_enabled(!is_builtin(&self.selected) && self.load_error.is_none(), secondary_button("Delete saved profile")).clicked() {
                        if let Err(error) = self.delete_selected() {
                            self.note = Some((error, true));
                        } else {
                            actions.push(ProfileAction::AutomationChanged);
                        }
                    }
                });
            }
        });
        ui.add_space(12.0);
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, "Save current settings");
            ui.label("Stores confirmed fan values, CPU preset (when available), the last GPU level requested in this session and charging policy. Unreadable settings are left unchanged.");
            ui.horizontal_wrapped(|ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut self.new_name).hint_text("e.g. Desk or Travel").char_limit(64));
                if ui.add_enabled(can_capture && self.load_error.is_none() && !self.new_name.trim().is_empty(), secondary_button("Save current")).clicked() {
                    actions.push(ProfileAction::Capture(self.new_name.trim().to_string()));
                }
            });
            ui.label(RichText::new("An existing custom profile with this name is replaced; built-in profiles are protected.").color(MUTED));
        });
        ui.add_space(12.0);
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, "Automatic switching");
            ui.label("Automation runs while this app is open. It starts disabled and never runs a temperature-based fan loop.");
            ui.add_enabled_ui(self.load_error.is_none(), |ui| {
                ui.checkbox(&mut self.startup_draft, "Apply saved profile when the app opens");
                ui.checkbox(&mut self.switch_draft, "Switch profile when the charger is connected or disconnected");
                ui.checkbox(&mut self.resume_draft, "Reapply the saved profile after sleep");
                ui.checkbox(&mut self.tray_draft, "Keep running in the notification area when the window is closed");
                if !self.tray_draft {
                    self.minimized_draft = false;
                }
                ui.add_enabled_ui(self.tray_draft, |ui| {
                    ui.checkbox(&mut self.minimized_draft, "Start in the notification area")
                        .on_hover_text("The window hides only after Windows confirms that its notification icon is available.");
                });
                ui.checkbox(&mut self.hotkey_draft, "Enable the laptop fan Fn hotkey (Max / previous fan mode)")
                    .on_hover_text("If no previous mode has been confirmed in this app, the hotkey returns to Normal.");
                ui.checkbox(&mut self.wifi_hotkey_draft, "Enable the laptop Wi-Fi Fn hotkey")
                    .on_hover_text("Toggles Wi-Fi adapters using their current Windows state. Bluetooth is unchanged. The first change may request Windows radio permission.");
                ui.checkbox(&mut self.display_off_hotkey_draft, "Use Fn+F6 to turn off the display")
                    .on_hover_text("Explicitly selects display off for this key. It does not infer a panel-specific lock action or change screen brightness.");
                let names: Vec<String> = self.saved.config.profiles.iter().map(|profile| profile.name.clone()).collect();
                power_profile_choice(ui, "charger_profile", "On charger", &mut self.ac_draft, &names);
                power_profile_choice(ui, "battery_profile", "On battery", &mut self.dc_draft, &names);
                if ui.add(secondary_button("Save automation")).clicked() {
                    let mut next = self.saved.clone();
                    next.apply_at_startup = self.startup_draft;
                    next.switch_on_power_change = self.switch_draft;
                    next.reapply_on_resume = self.resume_draft;
                    next.keep_in_tray = self.tray_draft;
                    next.start_minimized = self.tray_draft && self.minimized_draft;
                    next.fan_hotkey = self.hotkey_draft;
                    next.wifi_hotkey = self.wifi_hotkey_draft;
                    next.display_off_hotkey = self.display_off_hotkey_draft;
                    next.config.ac_profile = self.ac_draft.clone();
                    next.config.dc_profile = self.dc_draft.clone();
                    match self.commit(next) {
                        Ok(()) => {
                            self.note = Some(("Automation settings saved. No profile was applied by saving.".into(), false));
                            actions.push(ProfileAction::AutomationChanged);
                        }
                        Err(error) => self.note = Some((error, true)),
                    }
                }
            });
        });
        if let Some((text, error)) = &self.note {
            ui.add_space(10.0);
            if *error {
                banner(ui, text, DANGER);
            } else {
                ui.label(text);
            }
        }
        if let Some(path) = &self.path {
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("Saved in {}", path.display()))
                    .small()
                    .color(MUTED),
            );
        } else if self.load_error.is_none() {
            ui.label(
                RichText::new("Mock mode: profiles stay in memory for this session.").color(MUTED),
            );
        }
        actions
    }
}

fn is_builtin(name: &str) -> bool {
    aero_core::builtin_config()
        .profiles
        .iter()
        .any(|profile| profile.name.eq_ignore_ascii_case(name))
}

fn power_profile_choice(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    selected: &mut Option<String>,
    names: &[String],
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(id)
            .selected_text(selected.as_deref().unwrap_or("Last applied profile"))
            .show_ui(ui, |ui| {
                ui.selectable_value(selected, None, "Last applied profile");
                for name in names {
                    ui.selectable_value(selected, Some(name.clone()), name);
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_exact_curve_without_applying_or_changing_active_profile() {
        let mut tab = ProfilesTab::new(true);
        let mut profile = tab.saved.config.profile("Deep Control").unwrap().clone();
        profile.cpu_power_preset = Some(aero_core::CpuPowerPreset::Battery);
        profile.name = "Desk".into();
        tab.capture(profile.clone()).unwrap();
        assert_eq!(tab.saved.config.profile("Desk"), Some(&profile));
        assert_eq!(tab.saved.config.active, "Normal");
        assert_eq!(
            tab.saved
                .config
                .profile("Desk")
                .unwrap()
                .fan_curve()
                .unwrap()
                .points()[1]
                .duty_raw,
            68
        );
        assert!(tab
            .capture(aero_core::builtin_config().profiles[0].clone())
            .is_err());
    }

    #[test]
    fn invalid_loaded_file_blocks_every_automation_entry_point() {
        let mut tab = ProfilesTab::new(true);
        tab.saved.apply_at_startup = true;
        tab.saved.switch_on_power_change = true;
        tab.saved.reapply_on_resume = true;
        tab.load_error = Some("Invalid file".into());
        assert!(tab.startup_profile(Some(true)).is_none());
        assert!(tab.automation_profile(true).is_none());
        assert!(tab.resume_profile(Some(false)).is_none());
    }

    #[test]
    fn automatic_selection_uses_saved_power_bindings() {
        let mut tab = ProfilesTab::new(true);
        assert!(tab.automation_profile(true).is_none());
        tab.saved.switch_on_power_change = true;
        tab.saved.reapply_on_resume = true;
        assert_eq!(tab.automation_profile(true).unwrap().name, "Gaming");
        assert_eq!(tab.resume_profile(Some(false)).unwrap().name, "Quiet");
        assert!(tab.startup_profile(Some(true)).is_none());
    }
}
