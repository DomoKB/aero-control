//! Explicit refresh-rate changes and opt-in power-source switching.

use crate::display_controls::{DisplayControls, DisplaySettings};
use eframe::egui::{self, RichText};

use super::{card, primary_button, section_title, MUTED, TEXT, WARN};

pub struct DisplayTab {
    backend: DisplayControls,
    selected: Option<String>,
    draft_hz: Option<u32>,
    settings: DisplaySettings,
    draft_settings: DisplaySettings,
    settings_changed: bool,
    on_ac: Option<bool>,
    loaded: bool,
    status: Option<(String, bool)>,
}

impl DisplayTab {
    pub fn new(mock: bool) -> Self {
        Self {
            backend: DisplayControls::new(mock),
            selected: None,
            draft_hz: None,
            settings: DisplaySettings::default(),
            draft_settings: DisplaySettings::default(),
            settings_changed: false,
            on_ac: None,
            loaded: false,
            status: None,
        }
    }

    pub fn settings(&self) -> &DisplaySettings {
        &self.settings
    }

    /// Loading preferences never writes a display mode. A power-source update
    /// subsequently requests the configured mode, if switching was opted in.
    pub fn set_settings(&mut self, settings: DisplaySettings) -> Result<(), String> {
        settings.validate()?;
        self.selected = settings.display_id.clone();
        self.settings = settings.clone();
        self.draft_settings = settings;
        self.settings_changed = false;
        Ok(())
    }

    pub fn take_settings_changed(&mut self) -> bool {
        std::mem::take(&mut self.settings_changed)
    }

    /// Apply an opted-in rule only after the parent successfully saved it.
    pub fn settings_saved(&mut self) {
        self.apply_power_setting();
    }

    pub fn refresh(&mut self) {
        if let Some(status) = self.backend.poll() {
            self.status = Some(status);
        }
        if !self.backend.busy() {
            if let Err(error) = self.backend.refresh() {
                self.status = Some((error, true));
            }
        }
        self.loaded = true;
    }

    fn apply_power_setting(&mut self) {
        if let Some((id, hz)) = self.settings.target(self.on_ac) {
            if let Err(error) = self.backend.apply(id, hz, true) {
                self.status = Some((error, true));
            }
        }
    }

    /// Call from the app update path, including while this page is hidden. The
    /// worker only acts when the known power source changes, with no timer reads.
    pub fn on_power_source_changed(&mut self, on_ac: Option<bool>) -> Option<(String, bool)> {
        if let Some(status) = self.backend.poll() {
            self.status = Some(status);
        }
        if self.on_ac != on_ac {
            self.on_ac = on_ac;
            self.apply_power_setting();
        }
        self.status.take()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<(String, bool)> {
        if let Some(status) = self.backend.poll() {
            self.status = Some(status);
        }
        if !self.loaded {
            self.refresh();
        }
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, "Display refresh rate");
            ui.label(RichText::new("Choose a rate reported by Windows at the current resolution, colour depth and orientation. The screen may briefly blink.").color(MUTED));
            if self.backend.busy() {
                ui.horizontal(|ui| { ui.spinner(); ui.label("Reading or changing the display mode…"); });
            }
            if ui.add_enabled(!self.backend.busy(), egui::Button::new("Refresh displays")).clicked() { self.refresh(); }
            let displays = match self.backend.displays.clone() {
                Ok(displays) => displays,
                Err(error) => { ui.colored_label(WARN, error); return; }
            };
            if displays.is_empty() { ui.label("No attached display mode was found."); return; }
            if !displays.iter().any(|display| Some(&display.id) == self.selected.as_ref()) {
                self.selected = Some(displays[0].id.clone()); self.draft_hz = None;
            }
            let previous = self.selected.clone();
            let selected_label = displays.iter().find(|display| Some(&display.id) == self.selected.as_ref())
                .map(|display| format!("{} · {}", display.label, display.device_name)).unwrap_or_default();
            egui::ComboBox::from_id_salt("refresh_display").selected_text(selected_label).show_ui(ui, |ui| {
                for display in &displays {
                    ui.selectable_value(&mut self.selected, Some(display.id.clone()), format!("{} · {}", display.label, display.device_name));
                }
            });
            if previous != self.selected { self.draft_hz = None; }
            let display = displays.iter().find(|display| Some(&display.id) == self.selected.as_ref()).unwrap();
            ui.label(RichText::new(format!("Confirmed: {} × {} · {}-bit · {} Hz", display.current.width, display.current.height, display.current.bits_per_pixel, display.current.hz)).color(TEXT));
            if self.draft_hz.is_none() { self.draft_hz = Some(display.current.hz); }
            frequency_choice(ui, "manual_display_hz", "Refresh rate", &display.frequencies, &mut self.draft_hz);
            let can_apply = !self.backend.busy() && self.draft_hz.is_some_and(|hz| hz != display.current.hz && display.frequencies.contains(&hz));
            if ui.add_enabled(can_apply, primary_button("Apply refresh rate")).clicked() {
                if let Err(error) = self.backend.apply(&display.id, self.draft_hz.unwrap(), false) { self.status = Some((error, true)); }
            }
            if display.frequencies.len() <= 1 { ui.label(RichText::new("Windows reports no alternative refresh rate for this mode.").color(MUTED)); }
            ui.add_space(14.0);
            section_title(ui, "Switch with power source");
            ui.label(RichText::new("Optional. Applies only to this monitor while aero-control runs. Display settings are not changed when the power source is unknown.").color(MUTED));
            if !display.stable_identity {
                ui.colored_label(WARN, "Windows did not supply a stable monitor identity. Automatic switching is unavailable for this display.");
            }
            if self.draft_settings.display_id.as_deref() != Some(display.id.as_str()) {
                self.draft_settings.display_id = Some(display.id.clone());
                self.draft_settings.ac_hz = None; self.draft_settings.dc_hz = None;
            }
            ui.add_enabled_ui(display.stable_identity, |ui| {
                ui.checkbox(&mut self.draft_settings.automatic, "Automatically switch this display on charger / battery changes");
                frequency_choice(ui, "display_ac_hz", "On charger", &display.frequencies, &mut self.draft_settings.ac_hz);
                frequency_choice(ui, "display_dc_hz", "On battery", &display.frequencies, &mut self.draft_settings.dc_hz);
            });
            let validation = self.draft_settings.validate().and_then(|()| {
                if self.draft_settings.automatic && (!display.stable_identity || [self.draft_settings.ac_hz, self.draft_settings.dc_hz].into_iter().flatten().any(|hz| !display.frequencies.contains(&hz))) {
                    Err("Choose rates supported by the selected display's current mode.".into())
                } else { Ok(()) }
            });
            if let Err(error) = &validation { ui.colored_label(WARN, error); }
            if ui.add_enabled(!self.backend.busy() && validation.is_ok(), egui::Button::new("Save display switching")).clicked() {
                self.settings = self.draft_settings.clone(); self.settings_changed = true;
                self.status = Some(("Display switching preference updated.".into(), false));
            }
            if self.backend.is_mock() { ui.label(RichText::new("Mock display: no Windows display mode is read or changed.").color(MUTED)); }
        });
        self.status.take()
    }
}

fn frequency_choice(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    rates: &[u32],
    selected: &mut Option<u32>,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(id)
            .selected_text(
                selected
                    .map(|hz| format!("{hz} Hz"))
                    .unwrap_or_else(|| "Choose rate".into()),
            )
            .show_ui(ui, |ui| {
                for hz in rates {
                    ui.selectable_value(selected, Some(*hz), format!("{hz} Hz"));
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_automation_is_opt_in_and_ignores_unknown_power() {
        let mut tab = DisplayTab::new(true);
        tab.refresh();
        let id = tab.backend.displays.as_ref().unwrap()[0].id.clone();
        tab.on_power_source_changed(Some(true));
        assert_eq!(tab.backend.displays.as_ref().unwrap()[0].current.hz, 60);
        tab.set_settings(DisplaySettings {
            automatic: true,
            display_id: Some(id),
            ac_hz: Some(144),
            dc_hz: Some(60),
        })
        .unwrap();
        assert_eq!(tab.backend.displays.as_ref().unwrap()[0].current.hz, 60);
        tab.settings_saved();
        assert_eq!(tab.backend.displays.as_ref().unwrap()[0].current.hz, 144);
        tab.on_power_source_changed(None);
        assert_eq!(tab.backend.displays.as_ref().unwrap()[0].current.hz, 144);
        tab.on_power_source_changed(Some(true));
        assert_eq!(tab.backend.displays.as_ref().unwrap()[0].current.hz, 144);
        tab.on_power_source_changed(Some(false));
        assert_eq!(tab.backend.displays.as_ref().unwrap()[0].current.hz, 60);
        assert!(!tab.take_settings_changed());
    }
    #[test]
    fn unsupported_saved_frequency_cannot_change_a_mock_display() {
        let mut tab = DisplayTab::new(true);
        tab.refresh();
        tab.set_settings(DisplaySettings {
            automatic: true,
            display_id: Some("mock-internal-display".into()),
            ac_hz: Some(240),
            dc_hz: Some(60),
        })
        .unwrap();
        let status = tab.on_power_source_changed(Some(true)).unwrap();
        assert!(status.1);
        assert_eq!(tab.backend.displays.as_ref().unwrap()[0].current.hz, 60);
    }
}
