use aero_core::CpuPowerPreset;
use eframe::egui::{self, RichText};

use super::{card, primary_button, secondary_button, section_title, DANGER, MUTED};
use crate::cpu_power::CpuPowerSnapshot;

pub struct CpuPowerTab {
    reading: Option<Result<CpuPowerSnapshot, String>>,
    draft: CpuPowerPreset,
    refresh_needed: bool,
}

impl Default for CpuPowerTab {
    fn default() -> Self {
        Self {
            reading: None,
            draft: CpuPowerPreset::Level3,
            refresh_needed: false,
        }
    }
}

impl CpuPowerTab {
    pub fn enter(&mut self) {
        self.refresh_needed = true;
    }

    pub fn observe(&mut self, reading: Result<CpuPowerSnapshot, String>) {
        if let Ok(snapshot) = &reading {
            if let Some(preset) = snapshot.preset() {
                self.draft = preset;
            }
        }
        self.reading = Some(reading);
    }

    pub fn confirmed_preset(&self) -> Option<CpuPowerPreset> {
        self.reading.as_ref()?.as_ref().ok()?.preset()
    }

    /// Outer Some is an action; inner None requests a read without applying.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        can_write: bool,
        busy: bool,
        identity: &aero_wmi::SkuInfo,
        confirmed_model: bool,
    ) -> Option<Option<CpuPowerPreset>> {
        let model = if crate::device_identity::uses_confirmed_yb(identity, confirmed_model) {
            "AERO 17 YB (selected)"
        } else {
            crate::device_identity::model_name(identity)
        };
        if !crate::device_identity::supports_yb(identity, confirmed_model) {
            self.refresh_needed = false;
            card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                section_title(ui, "CPU power limits");
                ui.label(format!(
                    "No verified CPU power presets are available for {model}."
                ));
            });
            return None;
        }
        let mut action = None;
        if self.refresh_needed && !busy {
            self.refresh_needed = false;
            action = Some(None);
        }
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, "CPU power limits");
            ui.label(format!("Official Control Center presets for {model}. Requires a Core i7-10875H and a compatible Intel XTU installation."));
            ui.add_space(8.0);
            match &self.reading {
                Some(Ok(snapshot)) => {
                    ui.label(format!("Confirmed: {:.2} W sustained · {:.2} W short burst", snapshot.pl1_mw as f64 / 1000.0, snapshot.pl2_mw as f64 / 1000.0));
                    if snapshot.preset().is_none() { ui.label(RichText::new("Current limits do not match an official preset.").color(MUTED)); }
                }
                Some(Err(error)) => { ui.label(RichText::new(error).color(DANGER)); }
                None => { ui.label(RichText::new("CPU limits have not been read.").color(MUTED)); }
            }
            ui.add_space(10.0);
            ui.add_enabled_ui(!busy, |ui| {
                egui::ComboBox::from_id_salt("cpu_power_preset")
                    .selected_text(self.draft.label())
                    .show_ui(ui, |ui| {
                        for preset in CpuPowerPreset::ALL {
                            let available = self.reading.as_ref().and_then(|r| r.as_ref().ok()).is_some_and(|s| s.supports(preset));
                            ui.add_enabled_ui(available, |ui| { ui.selectable_value(&mut self.draft, preset, preset.label()); });
                        }
                    });
            });
            let (pl1, pl2) = self.draft.limits_mw();
            ui.label(RichText::new(format!("Selected: {:.2} W sustained · {:.2} W short burst", pl1 as f64 / 1000.0, pl2 as f64 / 1000.0)).color(MUTED));
            let available = self.reading.as_ref().and_then(|r| r.as_ref().ok()).is_some_and(|s| s.supports(self.draft));
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(can_write && available && !busy && action.is_none(), primary_button("Apply CPU preset")).clicked() { action = Some(Some(self.draft)); }
                if ui.add_enabled(!busy && action.is_none(), secondary_button("Refresh CPU limits")).clicked() { action = Some(None); }
            });
            ui.label(RichText::new("Sustained limits longer workloads; short burst limits brief turbo boosts. These are power caps, not measured consumption. CPU tuning requires a compatible installed Intel XTU SDK and service. Locked presets are unavailable.").color(MUTED));
        });
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_model_cannot_offer_cpu_actions_from_a_previous_reading() {
        let ctx = egui::Context::default();
        let mut identity = crate::device_identity::read(true);
        identity.model = Some("AERO 15 XE4".into());
        identity.system_sku = Some("P65XE4".into());
        let mut tab = CpuPowerTab::default();
        tab.observe(crate::cpu_power::CpuPower::new(true, true, false).read());
        tab.enter();
        let mut action = None;
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                action = tab.ui(ui, true, false, &identity, true);
            });
        });
        assert_eq!(action, None);
        assert!(!tab.refresh_needed);
    }

    #[test]
    fn opening_requests_one_read_and_idle_frames_never_apply_or_poll() {
        let ctx = egui::Context::default();
        let mut tab = CpuPowerTab::default();
        tab.enter();
        for expected in [Some(None), None, None] {
            let mut action = None;
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    action = tab.ui(ui, true, false, &crate::device_identity::read(true), false);
                });
            });
            assert_eq!(action, expected);
        }
    }
}
