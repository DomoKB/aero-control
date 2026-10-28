//! Keyboard lighting tab: Control Center's built-in keyboard options, applied
//! only when the user presses Apply, with 0x88 readback as the success test.

use std::path::PathBuf;

use aero_kbd::mock::MockKeyboard;
use aero_kbd::profile::{Store, PROFILE_COUNT};
use aero_kbd::proto::{self, DirectionKind};
use aero_kbd::{Color, Direction, Effect, KbdError, KeyboardBackend, LightSettings, Rgb, Verified};
use eframe::egui::{self, Color32, Frame, Margin, RichText, Rounding, Stroke, Vec2};

use super::{
    banner, info_card, ACCENT, CARD, CARD_RAISED, DANGER, MUTED, ON_ACCENT, STROKE, TEXT, WARN,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorMode {
    On,
    Random,
    Off,
}

pub struct KeyboardTab {
    mock: bool,
    backend: Option<Box<dyn KeyboardBackend>>,
    open_error: Option<String>,
    opened: bool,
    settings: LightSettings,
    /// Control Center's "oldcolor": the fixed colour to return to after Random.
    old_color: Color,
    /// Brightness to restore when switching Off back to On/Random.
    last_brightness: u8,
    store: Store,
    store_path: Option<PathBuf>,
    keyboard_state: Option<LightSettings>,
    custom_rgb: [u8; 3],
    status: Option<(String, bool)>,
    status_fresh: bool,
}

fn store_path(mock: bool) -> Option<PathBuf> {
    if mock {
        return None;
    }
    std::env::var_os("APPDATA").map(|d| {
        PathBuf::from(d)
            .join("aero-control")
            .join("keyboard-lighting.txt")
    })
}

impl KeyboardTab {
    pub fn new(mock: bool) -> Self {
        let store_path = store_path(mock);
        let store = store_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| Store::parse(&t))
            .unwrap_or_default();
        let settings = store.recall(Effect::Static).0;
        KeyboardTab {
            mock,
            backend: None,
            open_error: None,
            opened: false,
            settings,
            old_color: Color::Orange,
            last_brightness: proto::BRIGHTNESS_MAX,
            store,
            store_path,
            keyboard_state: None,
            custom_rgb: [255, 0, 0],
            status: None,
            status_fresh: false,
        }
    }

    /// Open the device the first time the tab is shown. Opening sends nothing except
    /// Control Center's firmware-version query (0x80).
    fn ensure_open(&mut self) {
        if self.opened {
            return;
        }
        self.opened = true;
        if self.mock {
            self.backend = Some(Box::new(MockKeyboard::new()));
            return;
        }
        match aero_kbd::open_real() {
            Ok(b) => self.backend = Some(b),
            Err(e) => self.open_error = Some(e.to_string()),
        }
    }

    fn save_store(&mut self) -> Result<(), String> {
        let Some(path) = &self.store_path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, self.store.serialize()).map_err(|e| e.to_string())
    }

    fn set_status(&mut self, text: impl Into<String>, error: bool) {
        self.status = Some((text.into(), error));
        self.status_fresh = true;
    }

    fn color_mode(&self) -> ColorMode {
        if self.settings.brightness == 0 {
            ColorMode::Off
        } else if self.settings.color == Color::Random {
            ColorMode::Random
        } else {
            ColorMode::On
        }
    }

    fn select_effect(&mut self, effect: Effect) {
        if effect == self.settings.effect {
            return;
        }
        self.store.remember(self.settings, self.old_color);
        let (s, old) = self.store.recall(effect);
        self.settings = s;
        self.old_color = old;
        if s.brightness > 0 {
            self.last_brightness = s.brightness;
        }
    }

    fn select_mode(&mut self, mode: ColorMode) {
        match mode {
            ColorMode::Off => {
                if self.settings.brightness > 0 {
                    self.last_brightness = self.settings.brightness;
                }
                self.settings.brightness = 0;
            }
            ColorMode::Random => {
                if self.settings.color != Color::Random {
                    self.old_color = self.settings.color;
                }
                self.settings.color = Color::Random;
                if self.settings.brightness == 0 {
                    self.settings.brightness = self.last_brightness.max(1);
                }
            }
            ColorMode::On => {
                if self.settings.color == Color::Random {
                    self.settings.color = self.old_color;
                }
                if self.settings.color == Color::Random {
                    self.settings.color = Color::Orange;
                }
                if self.settings.brightness == 0 {
                    self.settings.brightness = self.last_brightness.max(1);
                }
            }
        }
    }

    fn select_color(&mut self, color: Color) {
        self.settings.color = color;
        self.old_color = color;
        // Control Center: picking a swatch while Off turns the light back on at 50.
        if self.settings.brightness == 0 {
            self.settings.brightness = proto::BRIGHTNESS_MAX;
        }
    }

    fn apply(&mut self) {
        let settings = self.settings.normalized();
        if let Err(e) = settings.validate() {
            self.set_status(format!("Not sent: {e}"), true);
            return;
        }
        let Some(kb) = self.backend.as_mut() else {
            self.set_status("Not sent: no keyboard backend.", true);
            return;
        };
        let label = kb.label();
        match kb.apply(&settings) {
            Ok(Verified::ReadBack(read)) => {
                self.keyboard_state = Some(read);
                self.store.remember(settings, self.old_color);
                let saved = self.save_store();
                let mut msg = format!("{label}: keyboard confirmed {}", read.describe());
                if let Err(e) = saved {
                    msg.push_str(&format!(" (settings file not saved: {e})"));
                }
                self.set_status(msg, false);
            }
            Ok(Verified::WriteOnly) => {
                self.set_status(
                    format!("{label}: write accepted; this backend cannot read back."),
                    false,
                );
            }
            Err(e) => {
                if let KbdError::Mismatch { read, .. } = &e {
                    self.keyboard_state = Some(*read);
                }
                self.set_status(format!("{label}: {e}"), true);
            }
        }
    }

    fn read_back(&mut self) {
        let Some(kb) = self.backend.as_mut() else {
            self.set_status("No keyboard backend.", true);
            return;
        };
        match kb.read() {
            Ok(s) => {
                self.keyboard_state = Some(s);
                self.set_status(format!("Keyboard reports {}", s.describe()), false);
            }
            Err(e) => self.set_status(format!("Read failed: {e}"), true),
        }
    }

    fn fill_custom(&mut self, slot: u8) {
        let Some(kb) = self.backend.as_mut() else {
            self.set_status("No keyboard backend.", true);
            return;
        };
        let [r, g, b] = self.custom_rgb;
        let matrix = proto::solid_matrix(Rgb(r, g, b));
        match kb.write_custom(slot, &matrix) {
            Ok(()) => self.set_status(
                format!("CUSTOM {} picture written and read back (all keys #{r:02X}{g:02X}{b:02X}). Press Apply to show it.", slot + 1),
                false,
            ),
            Err(e) => self.set_status(format!("CUSTOM {} picture: {e}", slot + 1), true),
        }
    }

    /// The latest result, once, for the window's status line.
    pub fn new_status(&mut self) -> Option<(String, bool)> {
        if !std::mem::take(&mut self.status_fresh) {
            return None;
        }
        self.status.clone()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.ensure_open();
        ui.label(RichText::new("Keyboard").size(22.0).strong().color(TEXT));
        ui.label(
            RichText::new(
                "Built-in keyboard lighting, as in Control Center. Nothing is sent until Apply.",
            )
            .color(MUTED),
        );
        ui.add_space(12.0);

        match (&self.backend, &self.open_error) {
            (Some(kb), _) => {
                let i = kb.info();
                let fw = i.firmware.clone().unwrap_or_else(|| "unknown".into());
                info_card(
                    ui,
                    &format!(
                        "{} · {:04X}:{:04X} · firmware {fw}",
                        kb.label(),
                        i.vendor_id,
                        i.product_id
                    ),
                    &i.path,
                );
            }
            (None, Some(e)) => banner(ui, &format!("Keyboard unavailable: {e}"), DANGER),
            (None, None) => {}
        }
        ui.add_space(12.0);

        section(ui, "LIGHTING EFFECT");
        let mut picked = None;
        ui.horizontal_wrapped(|ui| {
            for effect in Effect::ALL {
                if tile(ui, effect.gcc_name(), self.settings.effect == effect, 150.0).clicked() {
                    picked = Some(effect);
                }
            }
        });
        if let Some(e) = picked {
            self.select_effect(e);
        }
        ui.add_space(12.0);

        let effect = self.settings.effect;
        card(ui, |ui| {
            if effect.is_custom() {
                self.custom_controls(ui);
            } else {
                self.color_controls(ui);
            }
            ui.add_space(8.0);
            let mut pct = proto::brightness_to_slider(self.settings.brightness);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Brightness").color(TEXT));
                if ui
                    .add(
                        egui::Slider::new(&mut pct, 0..=100)
                            .step_by(2.0)
                            .suffix("%"),
                    )
                    .changed()
                {
                    self.settings.brightness = proto::brightness_from_slider(pct);
                }
                ui.label(
                    RichText::new(format!("raw {}/50", self.settings.brightness)).color(MUTED),
                );
            });
            if effect.shows_speed(self.settings.color) {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Speed").color(TEXT));
                    // Firmware speed 1 is fastest; show it the way Control Center's slider reads.
                    let mut fast = 11 - self.settings.speed;
                    if ui
                        .add(egui::Slider::new(&mut fast, 1..=10).text("Min → Max"))
                        .changed()
                    {
                        self.settings.speed = 11 - fast;
                    }
                    ui.label(
                        RichText::new(format!("raw {} (1 = fastest)", self.settings.speed))
                            .color(MUTED),
                    );
                });
            }
            let kind = effect.direction_kind();
            if kind != DirectionKind::None {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Direction").color(TEXT));
                    let options: &[Direction] = if kind == DirectionKind::Rotate {
                        &Direction::LINEAR[..2]
                    } else {
                        &Direction::LINEAR
                    };
                    for d in options {
                        if tile(ui, d.label(kind), self.settings.direction == *d, 130.0).clicked() {
                            self.settings.direction = *d;
                        }
                    }
                });
            }
        });
        ui.add_space(12.0);

        ui.horizontal(|ui| {
            let ready = self.backend.is_some();
            let apply = egui::Button::new(RichText::new("Apply").strong().color(ON_ACCENT))
                .fill(ACCENT)
                .rounding(Rounding::same(10.0))
                .min_size(Vec2::new(120.0, 38.0));
            if ui.add_enabled(ready, apply).clicked() {
                self.apply();
            }
            let read = egui::Button::new("Read from keyboard")
                .fill(CARD_RAISED)
                .rounding(Rounding::same(10.0))
                .min_size(Vec2::new(160.0, 38.0));
            if ui.add_enabled(ready, read).clicked() {
                self.read_back();
            }
            if ui
                .add(
                    egui::Button::new("Reset effect to Control Center default")
                        .fill(CARD_RAISED)
                        .rounding(Rounding::same(10.0)),
                )
                .clicked()
            {
                self.settings = LightSettings::effect_default(self.settings.effect);
                self.old_color = Color::Orange;
            }
        });
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!(
                "Will send: {}",
                self.settings.normalized().describe()
            ))
            .color(MUTED),
        );
        if let Some(s) = &self.keyboard_state {
            ui.label(RichText::new(format!("Keyboard reports: {}", s.describe())).color(TEXT));
        }
        if let Some((text, err)) = &self.status {
            ui.add_space(6.0);
            banner(ui, text, if *err { DANGER } else { ACCENT });
        }
        ui.add_space(14.0);

        section(ui, "PROFILES");
        let mut load = None;
        let mut save = None;
        ui.horizontal_wrapped(|ui| {
            for i in 0..PROFILE_COUNT {
                let p = &self.store.profiles[i];
                card(ui, |ui| {
                    ui.set_width(200.0);
                    let on = self.store.current_profile == i;
                    ui.label(
                        RichText::new(&p.name)
                            .strong()
                            .color(if on { ACCENT } else { TEXT }),
                    );
                    ui.label(RichText::new(p.settings.describe()).size(12.0).color(MUTED));
                    ui.horizontal(|ui| {
                        if ui.button("Load").clicked() {
                            load = Some(i);
                        }
                        if ui.button("Save current").clicked() {
                            save = Some(i);
                        }
                    });
                });
            }
        });
        if let Some(i) = load {
            let s = self.store.profiles[i].settings;
            self.settings = s;
            if s.color != Color::Random {
                self.old_color = s.color;
            }
            self.store.current_profile = i;
            let saved = self.save_store();
            self.set_status(
                format!(
                    "{} loaded into the editor. Press Apply to send it.{}",
                    self.store.profiles[i].name,
                    saved
                        .err()
                        .map(|e| format!(" (file: {e})"))
                        .unwrap_or_default()
                ),
                false,
            );
        }
        if let Some(i) = save {
            self.store.profiles[i].settings = self.settings.normalized();
            self.store.current_profile = i;
            match self.save_store() {
                Ok(()) => self.set_status(
                    format!(
                        "{} saved (aero-control's file only).",
                        self.store.profiles[i].name
                    ),
                    false,
                ),
                Err(e) => self.set_status(format!("Profile not saved: {e}"), true),
            }
        }
        ui.add_space(12.0);
        egui::CollapsingHeader::new("About").default_open(false).show(ui, |ui| {
            ui.label("Targets only the built-in keyboard (USB 1044:7A3F on this Aero 17 YB; other Gigabyte Ite IDs 7A38/7A39/7A3A/7A3B are accepted) and only its collection with 9-byte feature reports, the same rule Control Center uses. External keyboards are never opened.");
            ui.add_space(4.0);
            ui.label("Apply sends one 0x08 feature report, waits 65 ms, then asks the keyboard what it is running (0x88). Success is reported only if every field reads back the same.");
            ui.add_space(4.0);
            ui.label("Profiles and per-effect memory live in %APPDATA%\\aero-control\\keyboard-lighting.txt. Control Center's registry and ProfileData.xml are not touched; the keyboard itself keeps the last applied effect.");
        });
    }

    fn color_controls(&mut self, ui: &mut egui::Ui) {
        let effect = self.settings.effect;
        let mode = self.color_mode();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Colour").color(TEXT));
            let modes: &[(ColorMode, &str)] = if effect.forces_random() {
                &[(ColorMode::Random, "Random"), (ColorMode::Off, "Off")]
            } else {
                &[
                    (ColorMode::On, "On"),
                    (ColorMode::Random, "Random"),
                    (ColorMode::Off, "Off"),
                ]
            };
            for (m, label) in modes {
                let on = mode == *m
                    || (effect.forces_random() && *m == ColorMode::Random && mode == ColorMode::On);
                if tile(ui, label, on, 90.0).clicked() {
                    self.select_mode(*m);
                }
            }
        });
        if effect.shows_palette() && mode == ColorMode::On {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                for c in Color::PALETTE {
                    if swatch(ui, c, self.settings.color == c).clicked() {
                        self.select_color(c);
                    }
                }
            });
        } else if effect.forces_random() {
            ui.label(
                RichText::new(format!(
                    "{effect} always runs Random colours in Control Center."
                ))
                .color(MUTED),
            );
        }
    }

    fn custom_controls(&mut self, ui: &mut egui::Ui) {
        let slot = self.settings.effect.custom_slot().unwrap_or(0);
        ui.label(
            RichText::new(format!(
                "CUSTOM {} plays the per-key picture stored in the keyboard. Apply selects it; brightness applies.",
                slot + 1
            ))
            .color(MUTED),
        );
        ui.horizontal(|ui| {
            ui.label(RichText::new("Fill colour").color(TEXT));
            ui.color_edit_button_srgb(&mut self.custom_rgb);
            let fill = egui::Button::new(format!("Write all keys of CUSTOM {}", slot + 1))
                .fill(CARD_RAISED)
                .rounding(Rounding::same(10.0));
            if ui.add_enabled(self.backend.is_some(), fill).clicked() {
                self.fill_custom(slot);
            }
        });
        ui.label(
            RichText::new("Per-key painting and Control Center's game pictures are not included; this writes one colour to every key slot and verifies it by reading the picture back.")
                .size(12.0)
                .color(WARN),
        );
    }
}

fn section(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(11.0).color(MUTED));
    ui.add_space(6.0);
}

fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    Frame::none()
        .fill(CARD)
        .rounding(Rounding::same(12.0))
        .stroke(Stroke::new(1.0_f32, STROKE))
        .inner_margin(Margin::same(14.0))
        .show(ui, add);
}

fn tile(ui: &mut egui::Ui, label: &str, on: bool, width: f32) -> egui::Response {
    let text = RichText::new(label).color(if on { ON_ACCENT } else { TEXT });
    ui.add(
        egui::Button::new(if on { text.strong() } else { text })
            .fill(if on { ACCENT } else { CARD_RAISED })
            .rounding(Rounding::same(10.0))
            .min_size(Vec2::new(width, 36.0)),
    )
}

fn swatch(ui: &mut egui::Ui, color: Color, on: bool) -> egui::Response {
    let Rgb(r, g, b) = color.preview();
    let fill = Color32::from_rgb(r, g, b);
    let text_color = if (r as u32 + g as u32 + b as u32) > 380 {
        Color32::BLACK
    } else {
        Color32::WHITE
    };
    ui.add(
        egui::Button::new(RichText::new(color.name()).color(text_color))
            .fill(fill)
            .stroke(Stroke::new(
                if on { 3.0_f32 } else { 1.0_f32 },
                if on { TEXT } else { STROKE },
            ))
            .rounding(Rounding::same(10.0))
            .min_size(Vec2::new(86.0, 36.0)),
    )
}
