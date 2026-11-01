//! Everyday system settings, with drafts kept separate from confirmed readings.

use crate::system_controls::{
    Brightness, SettingsPage, SystemControls, Volume, WirelessControls, WirelessState,
};
use eframe::egui::{self, RichText};

use super::{
    card, page_header, primary_button, secondary_button, section_title, MUTED, TEXT, WARN,
};

pub struct ControlsTab {
    backend: SystemControls,
    wireless: WirelessControls,
    volume: Result<Volume, String>,
    brightness: Result<Brightness, String>,
    volume_draft: u8,
    brightness_draft: u8,
    volume_dirty: bool,
    brightness_dirty: bool,
    readings_loaded: bool,
    status: Option<(String, bool)>,
}

impl ControlsTab {
    pub fn new(mock: bool) -> Self {
        Self {
            backend: SystemControls::new(mock),
            wireless: WirelessControls::new(mock),
            volume: Err("Not read yet.".into()),
            brightness: Err("Not read yet.".into()),
            volume_draft: 50,
            brightness_draft: 60,
            volume_dirty: false,
            brightness_dirty: false,
            readings_loaded: false,
            status: None,
        }
    }

    /// Read settings on page entry or explicit refresh, never on an idle timer.
    pub fn refresh_all(&mut self) {
        // An operation may have completed while a different page was open.
        if let Some(status) = self.wireless.poll() {
            self.status = Some(status);
        }
        self.volume = self.backend.volume();
        self.brightness = self.backend.brightness();
        if !self.volume_dirty {
            if let Ok(volume) = self.volume {
                self.volume_draft = volume.percent;
            }
        }
        if !self.brightness_dirty {
            if let Ok(brightness) = self.brightness {
                self.brightness_draft = brightness.percent;
            }
        }
        self.wireless.refresh();
        self.readings_loaded = true;
    }

    /// Drain completed work on any page. This never starts a hardware read.
    pub fn poll_background(&mut self) -> Option<(String, bool)> {
        if let Some(status) = self.wireless.poll() {
            self.status = Some(status);
        }
        self.status.take()
    }

    pub fn request_wifi_toggle(&mut self) -> Result<(), String> {
        self.wireless.toggle_wifi()?;
        self.status = Some((
            if self.backend.is_mock() {
                "Mock: Wi-Fi toggled.".into()
            } else {
                "Requesting Wi-Fi change…".into()
            },
            false,
        ));
        Ok(())
    }

    fn prefix(&self) -> &'static str {
        if self.backend.is_mock() {
            "Mock: "
        } else {
            ""
        }
    }

    /// Returns a fresh user-facing status for the parent app's status bar.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<(String, bool)> {
        if let Some(status) = self.wireless.poll() {
            self.status = Some(status);
        }
        if !self.readings_loaded {
            self.refresh_all();
        }
        page_header(
            ui,
            "System controls",
            "Readings update when you open this page, refresh, or change a setting here.",
        );
        if ui.button("Refresh readings").clicked() {
            self.refresh_all();
        }
        ui.add_space(12.0);
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, "Wi-Fi and Bluetooth");
            ui.label(RichText::new("Radio controls use Windows device permissions. A first change may open a Windows access prompt.").size(14.0).color(MUTED));
            ui.label(RichText::new("Shows adapter power state. Refresh radios to see changes made elsewhere; this does not search for nearby devices.").size(14.0).color(MUTED));
            if self.wireless.busy() {
                ui.horizontal(|ui| { ui.spinner(); ui.label("Reading or changing adapter state…"); });
            }
            match self.wireless.radios.clone() {
                Ok(radios) if radios.is_empty() => { ui.label("No compatible Wi-Fi or Bluetooth radio was found."); }
                Ok(radios) => {
                    for radio in radios {
                        ui.add_space(8.0);
                        ui.label(RichText::new(format!("{} · {}", radio.kind.label(), radio.state.label())).color(TEXT));
                        ui.label(RichText::new(&radio.name).size(13.0).color(MUTED));
                        let can_change = !self.wireless.busy() && matches!(radio.state, WirelessState::On | WirelessState::Off);
                        let turn_on = radio.state == WirelessState::Off;
                        if ui.add_enabled(can_change, secondary_button(if turn_on { "Turn on" } else { "Turn off" })).clicked() {
                            match self.wireless.set(radio.clone(), turn_on) {
                                Ok(()) if self.backend.is_mock() => self.status = Some((format!("Mock: {} {}.", radio.kind.label(), if turn_on { "enabled" } else { "disabled" }), false)),
                                Ok(()) => self.status = Some((format!("Requesting {} change…", radio.kind.label()), false)),
                                Err(error) => self.status = Some((error, true)),
                            }
                        }
                    }
                }
                Err(error) => { ui.colored_label(WARN, error); }
            }
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!self.wireless.busy(), secondary_button("Refresh radios")).clicked() {
                    self.wireless.refresh();
                }
                if ui.add_enabled(!self.wireless.busy(), egui::Button::new("Retry radio access")).on_hover_text("Clears the cached access result. Your next radio change requests permission again.").clicked() {
                    self.wireless.retry_access();
                    self.wireless.refresh();
                }
            });
        });
        ui.add_space(12.0);
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, "Default sound output");
            ui.spacing_mut().slider_width = (ui.available_width() - 170.0).clamp(180.0, 600.0);
            match self.volume.clone() {
                Ok(volume) => {
                    ui.label(
                        RichText::new(format!(
                            "Now: {}% · {}",
                            volume.percent,
                            if volume.muted { "muted" } else { "sound on" }
                        ))
                        .color(TEXT),
                    );
                    ui.add_space(8.0);
                    if ui
                        .add(
                            egui::Slider::new(&mut self.volume_draft, 0..=100)
                                .suffix("%")
                                .text("Volume"),
                        )
                        .changed()
                    {
                        self.volume_dirty = true;
                    }
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(self.volume_dirty, primary_button("Apply volume"))
                            .clicked()
                        {
                            match self.backend.set_volume(self.volume_draft) {
                                Ok(confirmed) => {
                                    self.volume = Ok(confirmed);
                                    self.volume_dirty = false;
                                    self.status = Some((
                                        format!(
                                            "{}Volume set to {}%.",
                                            self.prefix(),
                                            confirmed.percent
                                        ),
                                        false,
                                    ));
                                }
                                Err(error) => self.status = Some((error, true)),
                            }
                        }
                        if ui
                            .add(secondary_button(if volume.muted {
                                "Unmute"
                            } else {
                                "Mute"
                            }))
                            .clicked()
                        {
                            match self.backend.set_mute(!volume.muted) {
                                Ok(confirmed) => {
                                    self.volume = Ok(confirmed);
                                    self.status = Some((
                                        format!(
                                            "{}Output {}.",
                                            self.prefix(),
                                            if confirmed.muted { "muted" } else { "unmuted" }
                                        ),
                                        false,
                                    ));
                                }
                                Err(error) => self.status = Some((error, true)),
                            }
                        }
                    });
                }
                Err(error) => {
                    ui.colored_label(WARN, format!("Output unavailable: {error}"));
                }
            }
        });
        ui.add_space(12.0);
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, "Display brightness");
            ui.spacing_mut().slider_width = (ui.available_width() - 170.0).clamp(180.0, 600.0);
            match self.brightness.clone() {
                Ok(brightness) => {
                    ui.label(RichText::new(format!("Current {} power-plan setting: {}%", if brightness.on_ac { "AC" } else { "battery" }, brightness.percent)).color(TEXT));
                    ui.label(RichText::new("Applies to the active plan and current power source. Panel support depends on Windows and the display driver.").size(14.0).color(MUTED));
                    if ui.add(egui::Slider::new(&mut self.brightness_draft, 0..=100).suffix("%").text("Brightness")).changed() {
                        self.brightness_dirty = true;
                    }
                    if ui.add_enabled(self.brightness_dirty, primary_button("Apply brightness")).clicked() {
                        match self.backend.set_brightness(self.brightness_draft) {
                            Ok(confirmed) => {
                                self.brightness = Ok(confirmed);
                                self.brightness_dirty = false;
                                self.status = Some((format!("{}Brightness setting saved at {}%.", self.prefix(), confirmed.percent), false));
                            }
                            Err(error) => self.status = Some((error, true)),
                        }
                    }
                }
                Err(error) => { ui.colored_label(WARN, format!("Brightness unavailable: {error}")); }
            }
        });
        ui.add_space(12.0);
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, "Windows settings");
            ui.label(RichText::new("These shortcuts open Windows settings; they do not change devices or radios automatically.").size(14.0).color(MUTED));
            ui.horizontal_wrapped(|ui| {
                for page in SettingsPage::ALL {
                    if ui.add(secondary_button(page.label())).clicked() {
                        self.status = Some(match self.backend.open_settings(page) {
                            Ok(()) if self.backend.is_mock() => (format!("Mock preview: {}.", page.label()), false),
                            Ok(()) => (format!("Requested Windows to {}.", page.label().to_lowercase()), false),
                            Err(error) => (error, true),
                        });
                    }
                }
            });
        });
        self.status.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(tab: &mut ControlsTab, context: &egui::Context) {
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                tab.ui(ui);
            });
        });
    }

    #[test]
    fn idle_frames_keep_readings_until_explicit_refresh() {
        let mut tab = ControlsTab::new(true);
        let context = egui::Context::default();
        draw(&mut tab, &context);
        let initial_volume = tab.volume.clone().unwrap();
        let initial_brightness = tab.brightness.clone().unwrap();

        // Simulate changes in Windows while the System page remains open.
        let changed_volume = tab.backend.set_volume(73).unwrap();
        let changed_brightness = tab.backend.set_brightness(42).unwrap();
        // Cross the old polling interval, then render ordinary idle frames.
        std::thread::sleep(std::time::Duration::from_millis(2100));
        for _ in 0..3 {
            draw(&mut tab, &context);
        }
        assert_eq!(tab.volume.as_ref().unwrap(), &initial_volume);
        assert_eq!(tab.brightness.as_ref().unwrap(), &initial_brightness);
        assert_eq!(tab.volume_draft, initial_volume.percent);
        assert_eq!(tab.brightness_draft, initial_brightness.percent);

        tab.refresh_all();
        assert_eq!(tab.volume.as_ref().unwrap(), &changed_volume);
        assert_eq!(tab.brightness.as_ref().unwrap(), &changed_brightness);
        assert_eq!(tab.volume_draft, changed_volume.percent);
        assert_eq!(tab.brightness_draft, changed_brightness.percent);
    }

    #[test]
    fn wifi_hotkey_status_is_available_without_opening_system_page() {
        let mut tab = ControlsTab::new(true);
        tab.request_wifi_toggle().unwrap();
        assert!(!tab.readings_loaded);
        assert_eq!(
            tab.wireless.radios.as_ref().unwrap()[0].state,
            WirelessState::Off
        );
        assert_eq!(
            tab.poll_background(),
            Some(("Mock: Wi-Fi toggled.".into(), false))
        );
        assert_eq!(tab.poll_background(), None);
        assert!(!tab.readings_loaded);
    }
}
