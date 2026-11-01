//! Explicit firmware device controls. All reads and writes use the existing
//! serialized session; rendering this section never accesses hardware.

use crate::session::{PowerSnapshot, PowerWrite};
use eframe::egui::{self, RichText};

use super::{card, secondary_button, section_title, MUTED, TEXT, WARN};

#[derive(Default)]
pub struct Actions {
    pub write: Option<PowerWrite>,
    pub refresh: bool,
}

pub fn ui(
    ui: &mut egui::Ui,
    power: Option<&PowerSnapshot>,
    can_write: bool,
    can_refresh: bool,
) -> Actions {
    let mut actions = Actions::default();
    card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        section_title(ui, "Built-in camera and USB charging");
        ui.label(RichText::new("Values are confirmed by firmware readback. Refresh for changes made outside this app.").size(14.0).color(MUTED));
        ui.add_space(8.0);
        flag_row(ui, "Camera", power.map(|power| &power.camera), can_write, |enabled| {
            actions.write = Some(PowerWrite::Camera(enabled));
        });
        ui.label(RichText::new("Disabling the camera makes it unavailable to camera apps.").size(13.0).color(MUTED));
        ui.add_space(8.0);
        flag_row(ui, "USB charging during sleep", power.map(|power| &power.sleep_usb_charge), can_write, |enabled| {
            actions.write = Some(PowerWrite::SleepUsbCharge(enabled));
        });
        ui.add_space(8.0);
        flag_row(ui, "USB charging during hibernation", power.map(|power| &power.hibernation_usb_charge), can_write, |enabled| {
            actions.write = Some(PowerWrite::HibernationUsbCharge(enabled));
        });
        ui.label(RichText::new("Uses the laptop’s supported USB charging port; availability depends on its sleep state.").size(13.0).color(MUTED));
        ui.add_space(8.0);
        if ui.add_enabled(can_refresh, secondary_button("Refresh firmware readings")).clicked() {
            actions.refresh = true;
        }
    });
    ui.add_space(12.0);
    card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        section_title(ui, "Device information");
        match power.map(|power| &power.identity) {
            Some(Ok(info)) => {
                egui::Grid::new("firmware_device_identity")
                    .spacing([24.0, 8.0])
                    .show(ui, |ui| {
                        for (label, value) in [
                            ("Manufacturer", &info.manufacturer),
                            ("Model", &info.model),
                            ("SKU", &info.system_sku),
                            ("BIOS version", &info.bios_version),
                            ("Baseboard", &info.baseboard_product),
                        ] {
                            ui.label(RichText::new(label).color(MUTED));
                            ui.label(
                                RichText::new(value.as_deref().unwrap_or("Unavailable"))
                                    .color(TEXT),
                            );
                            ui.end_row();
                        }
                    });
            }
            Some(Err(error)) => {
                ui.colored_label(WARN, error);
            }
            None => {
                ui.label("Device information has not been read yet.");
            }
        }
    });
    actions
}

fn flag_row(
    ui: &mut egui::Ui,
    label: &str,
    reading: Option<&Result<u32, String>>,
    can_write: bool,
    mut request: impl FnMut(bool),
) {
    ui.horizontal_wrapped(|ui| match reading {
        Some(Ok(value @ (0 | 1))) => {
            let enabled = *value == 1;
            ui.label(
                RichText::new(format!("{label} · {}", if enabled { "On" } else { "Off" }))
                    .color(TEXT),
            );
            if ui
                .add_enabled(
                    can_write,
                    secondary_button(if enabled { "Turn off" } else { "Turn on" }),
                )
                .clicked()
            {
                request(!enabled);
            }
        }
        Some(Ok(value)) => {
            ui.colored_label(WARN, format!("{label}: unknown firmware value {value}"));
        }
        Some(Err(error)) => {
            ui.colored_label(WARN, format!("{label}: {error}"));
        }
        None => {
            ui.label(format!("{label}: not read yet"));
        }
    });
}
