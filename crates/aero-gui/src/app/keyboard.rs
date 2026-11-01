//! Keyboard lighting tab: Control Center's built-in keyboard options, applied
//! once per pick or slider release, with 0x88 readback as the success test.

use std::path::PathBuf;

use aero_kbd::mock::MockKeyboard;
use aero_kbd::profile::{Store, PROFILE_COUNT};
use aero_kbd::proto::{self, DirectionKind};
use aero_kbd::{Color, Direction, Effect, KbdError, KeyboardBackend, LightSettings, Rgb, Verified};
use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, RichText, Rounding, Sense, Stroke, Vec2,
};

use super::{
    banner, card, kbd_preview, mix, page_header, section_title, segmented, ACCENT, BG, CARD_RAISED,
    DANGER, MUTED, ON_ACCENT, STROKE, TEXT,
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
                format!("CUSTOM {} picture written and read back (all keys #{r:02X}{g:02X}{b:02X}). Pick it again to show it.", slot + 1),
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
        page_header(
            ui,
            "Keyboard",
            "Built-in keyboard lighting, as in Control Center. Each change applies when you pick it.",
        );
        if let Some(e) = &self.open_error {
            banner(ui, &format!("Keyboard unavailable: {e}"), DANGER);
            ui.add_space(12.0);
        }
        let wide = ui.available_width() > 720.0;
        if wide {
            self.preview_card(ui);
            ui.add_space(12.0);
        }
        self.effect_card(ui);
        ui.add_space(12.0);
        self.controls_card(ui);
        ui.add_space(12.0);
        if !wide {
            self.preview_card(ui);
            ui.add_space(12.0);
        }
        self.profiles(ui);
        ui.add_space(12.0);
        egui::CollapsingHeader::new("About").default_open(false).show(ui, |ui| {
            ui.label("Targets the compatible built-in Gigabyte ITE keyboard detected on this laptop (USB vendor 1044; product IDs 7A3F/7A38/7A39/7A3A/7A3B) and only its collection with 9-byte feature reports, the same rule Control Center uses. External keyboards are never opened.");
            ui.add_space(4.0);
            ui.label("Each change sends one 0x08 feature report, waits 65 ms, then asks the keyboard what it is running (0x88). Success is reported only if every field reads back the same. Sliders send once, when you let go.");
            ui.add_space(4.0);
            ui.label("Profiles and per-effect memory live in %APPDATA%\\aero-control\\keyboard-lighting.txt. Control Center's registry and ProfileData.xml are not touched; the keyboard itself keeps the last applied effect.");
        });
    }

    fn preview_card(&mut self, ui: &mut egui::Ui) {
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let width = (ui.available_width() * 0.55).min(460.0);
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(width, width * 0.3), Sense::hover());
                kbd_preview::paint(ui.painter(), rect, &self.settings, self.custom_rgb);
                ui.add_space(16.0);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(title_case(self.settings.effect.gcc_name()))
                            .size(18.0)
                            .strong()
                            .color(TEXT),
                    );
                    ui.label(
                        RichText::new(self.settings.normalized().describe())
                            .size(13.0)
                            .color(MUTED),
                    );
                    ui.add_space(6.0);
                    self.live_state(ui);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let ready = self.backend.is_some();
                        if ui
                            .add_enabled(ready, small_button("Read from keyboard"))
                            .clicked()
                        {
                            self.read_back();
                        }
                        if ui
                            .add_enabled(ready, small_button("Reset effect"))
                            .on_hover_text("Control Center's default for this effect.")
                            .clicked()
                        {
                            self.settings = LightSettings::effect_default(self.settings.effect);
                            self.old_color = Color::Orange;
                            self.apply();
                        }
                    });
                    if let Some(kb) = &self.backend {
                        let i = kb.info();
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!(
                                "{} · {:04X}:{:04X} · firmware {}",
                                kb.label(),
                                i.vendor_id,
                                i.product_id,
                                i.firmware.as_deref().unwrap_or("unknown")
                            ))
                            .size(12.0)
                            .color(MUTED),
                        );
                    }
                });
            });
        });
    }

    fn live_state(&self, ui: &mut egui::Ui) {
        let (text, color) = match (&self.status, self.keyboard_state) {
            (Some((_, true)), _) => ("Not applied, see the status line", DANGER),
            (_, Some(read)) if read == self.settings.normalized() => {
                ("Live · confirmed by the keyboard", ACCENT)
            }
            _ => ("Not applied yet", MUTED),
        };
        ui.horizontal(|ui| {
            let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().circle_filled(dot.center(), 4.0, color);
            ui.label(RichText::new(text).size(13.0).color(color));
        });
    }

    fn effect_card(&mut self, ui: &mut egui::Ui) {
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, "Effect");
            ui.add_space(10.0);
            let gap = 8.0;
            let compact = ui.available_width() < 720.0;
            let cols = ((ui.available_width() + gap) / 150.0).floor().max(1.0) as usize;
            let tile_w = (ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32;
            let mut picked = None;
            for row in Effect::ALL.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for effect in row {
                        if self.effect_tile(ui, *effect, tile_w, compact).clicked() {
                            picked = Some(*effect);
                        }
                    }
                });
                ui.add_space(gap - ui.spacing().item_spacing.y);
            }
            if let Some(effect) = picked {
                self.select_effect(effect);
                self.apply();
            }
        });
    }

    fn effect_tile(
        &self,
        ui: &mut egui::Ui,
        effect: Effect,
        width: f32,
        compact: bool,
    ) -> egui::Response {
        let height = if compact { 30.0 } else { 36.0 };
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
        let on = self.settings.effect == effect;
        let fill = if on {
            mix(CARD_RAISED, ACCENT, 0.22)
        } else if response.hovered() {
            mix(CARD_RAISED, TEXT, 0.06)
        } else {
            CARD_RAISED
        };
        let stroke = if on {
            Stroke::new(1.5_f32, ACCENT)
        } else {
            Stroke::new(1.0_f32, STROKE)
        };
        let painter = ui.painter_at(rect);
        painter.rect(rect.shrink(0.5), Rounding::same(10.0), fill, stroke);
        let chip = Rect::from_min_size(
            Pos2::new(rect.left() + 10.0, rect.center().y - 7.0),
            Vec2::new(20.0, 14.0),
        );
        let mut sample = self.store.recall(effect).0;
        sample.brightness = proto::BRIGHTNESS_MAX;
        if on {
            sample = self.settings;
            sample.brightness = sample.brightness.max(proto::BRIGHTNESS_MAX / 2);
        }
        for i in 0..4 {
            let part = Rect::from_min_size(
                chip.min + Vec2::new(i as f32 * 5.0, 0.0),
                Vec2::new(5.0, chip.height()),
            );
            let x = (i as f32 + 0.5) / 4.0;
            painter.rect_filled(
                part,
                Rounding::ZERO,
                kbd_preview::key_color(&sample, self.custom_rgb, x, 0.3),
            );
        }
        painter.rect_stroke(chip, Rounding::same(3.0), Stroke::new(1.0_f32, STROKE));
        painter.text(
            Pos2::new(chip.right() + 10.0, rect.center().y),
            Align2::LEFT_CENTER,
            title_case(effect.gcc_name()),
            FontId::proportional(if compact { 12.5 } else { 13.5 }),
            if on { TEXT } else { mix(MUTED, TEXT, 0.6) },
        );
        response
    }

    fn controls_card(&mut self, ui: &mut egui::Ui) {
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            if self.settings.effect.is_custom() {
                self.custom_controls(ui);
            } else {
                self.color_controls(ui);
            }
            ui.add_space(12.0);
            let mut pct = proto::brightness_to_slider(self.settings.brightness);
            let (changed, done) = slider(ui, "Brightness", &mut pct, 0, 100, "%");
            if changed {
                self.settings.brightness = proto::brightness_from_slider(pct);
            }
            if done {
                self.apply();
            }
            if self.settings.effect.shows_speed(self.settings.color) {
                ui.add_space(6.0);
                let mut fast = 11 - self.settings.speed;
                let (changed, done) = slider(ui, "Speed", &mut fast, 1, 10, " / 10");
                if changed {
                    self.settings.speed = 11 - fast;
                }
                if done {
                    self.apply();
                }
            }
            let kind = self.settings.effect.direction_kind();
            if kind != DirectionKind::None {
                ui.add_space(10.0);
                let options: &[Direction] = if kind == DirectionKind::Rotate {
                    &Direction::LINEAR[..2]
                } else {
                    &Direction::LINEAR
                };
                let items: Vec<_> = options.iter().map(|d| (d.label(kind), "", true)).collect();
                let on = options.iter().position(|d| *d == self.settings.direction);
                ui.horizontal(|ui| {
                    row_label(ui, "Direction", 46.0);
                    if let Some(index) = segmented(ui, "kbd_direction", &items, on, None) {
                        self.settings.direction = options[index];
                        self.apply();
                    }
                });
            }
        });
    }

    fn color_controls(&mut self, ui: &mut egui::Ui) {
        let effect = self.settings.effect;
        let mode = self.color_mode();
        let mut choice = None;
        ui.horizontal_wrapped(|ui| {
            row_label(ui, "Colour", 34.0);
            ui.spacing_mut().item_spacing.x = 10.0;
            if swatch(ui, Swatch::Off, mode == ColorMode::Off).clicked() {
                choice = Some(None);
            }
            let random_on =
                mode == ColorMode::Random || (effect.forces_random() && mode == ColorMode::On);
            if swatch(ui, Swatch::Random, random_on).clicked() {
                choice = Some(Some(Color::Random));
            }
            if effect.shows_palette() {
                ui.add_space(8.0);
                for c in Color::PALETTE {
                    let on = mode == ColorMode::On && self.settings.color == c;
                    if swatch(ui, Swatch::Palette(c), on).clicked() {
                        choice = Some(Some(c));
                    }
                }
            } else {
                ui.label(
                    RichText::new(format!("{effect} always runs Random colours."))
                        .size(13.0)
                        .color(MUTED),
                );
            }
        });
        match choice {
            Some(None) => self.select_mode(ColorMode::Off),
            Some(Some(Color::Random)) => self.select_mode(ColorMode::Random),
            Some(Some(c)) => {
                self.select_mode(ColorMode::On);
                self.select_color(c);
            }
            None => return,
        }
        self.apply();
    }

    fn custom_controls(&mut self, ui: &mut egui::Ui) {
        let slot = self.settings.effect.custom_slot().unwrap_or(0);
        ui.horizontal(|ui| {
            row_label(ui, "Fill colour", 34.0);
            ui.color_edit_button_srgb(&mut self.custom_rgb);
            ui.add_space(8.0);
            if ui
                .add_enabled(
                    self.backend.is_some(),
                    small_button(&format!("Write to all keys of CUSTOM {}", slot + 1)),
                )
                .on_hover_text(
                    "Writes one colour to every key slot of this picture and reads it back.",
                )
                .clicked()
            {
                self.fill_custom(slot);
            }
        });
        ui.label(
            RichText::new(format!(
                "CUSTOM {} plays the per-key picture stored in the keyboard. Per-key painting is not included.",
                slot + 1
            ))
            .size(12.0)
            .color(MUTED),
        );
    }

    fn profiles(&mut self, ui: &mut egui::Ui) {
        let mut load = None;
        let mut save = false;
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                section_title(ui, "Profiles");
                ui.add_space(8.0);
                for i in 0..PROFILE_COUNT {
                    let p = &self.store.profiles[i];
                    let on = self.store.current_profile == i;
                    let text = RichText::new(&p.name).color(if on { ON_ACCENT } else { TEXT });
                    if ui
                        .add(
                            egui::Button::new(text)
                                .fill(if on { ACCENT } else { CARD_RAISED })
                                .rounding(Rounding::same(10.0))
                                .min_size(Vec2::new(0.0, 32.0)),
                        )
                        .on_hover_text(format!("Load and apply: {}", p.settings.describe()))
                        .clicked()
                    {
                        load = Some(i);
                    }
                }
                if ui
                    .add(small_button("Save current here"))
                    .on_hover_text("Saves the current lighting into the selected profile (aero-control's file only).")
                    .clicked()
                {
                    save = true;
                }
            });
        });
        if let Some(i) = load {
            let s = self.store.profiles[i].settings;
            self.settings = s;
            if s.color != Color::Random {
                self.old_color = s.color;
            }
            self.store.current_profile = i;
            if let Err(e) = self.save_store() {
                self.set_status(format!("Profile choice not saved: {e}"), true);
            }
            self.apply();
        }
        if save {
            let i = self.store.current_profile;
            self.store.profiles[i].settings = self.settings.normalized();
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
    }
}

const LABEL_W: f32 = 92.0;

fn row_label(ui: &mut egui::Ui, text: &str, height: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(LABEL_W, height), Sense::hover());
    ui.painter().text(
        rect.left_center(),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(14.5),
        TEXT,
    );
}

fn title_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut start = true;
    for ch in name.chars() {
        if start {
            out.extend(ch.to_uppercase());
        } else {
            out.extend(ch.to_lowercase());
        }
        start = ch == ' ';
    }
    out
}

fn small_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).size(13.0).color(TEXT))
        .fill(CARD_RAISED)
        .rounding(Rounding::same(8.0))
        .stroke(Stroke::new(1.0_f32, STROKE))
        .min_size(Vec2::new(0.0, 28.0))
}

/// Painted slider. Returns (value changed, gesture finished with a change).
fn slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u8,
    min: u8,
    max: u8,
    unit: &str,
) -> (bool, bool) {
    let mut changed = false;
    let mut done = false;
    ui.horizontal(|ui| {
        row_label(ui, label, 30.0);
        let width = (ui.available_width() - 70.0).max(120.0);
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::click_and_drag());
        let track = Rect::from_center_size(rect.center(), Vec2::new(rect.width() - 16.0, 6.0));
        if let Some(pos) = response.interact_pointer_pos() {
            if response.dragged() || response.clicked() {
                let t = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
                let next = (f32::from(min) + t * f32::from(max - min)).round() as u8;
                if next != *value {
                    *value = next;
                    changed = true;
                }
            }
        }
        if response.drag_stopped() || response.clicked() {
            done = true;
        }
        let t = f32::from(*value - min) / f32::from(max - min);
        let painter = ui.painter();
        painter.rect_filled(track, Rounding::same(3.0), BG);
        let filled = Rect::from_min_max(
            track.min,
            Pos2::new(track.left() + track.width() * t, track.bottom()),
        );
        painter.rect_filled(filled, Rounding::same(3.0), ACCENT);
        let knob = Pos2::new(filled.right(), track.center().y);
        let ring = if response.hovered() || response.dragged() {
            ACCENT
        } else {
            STROKE
        };
        painter.circle(knob, 9.0, TEXT, Stroke::new(2.0_f32, ring));
        ui.add_space(8.0);
        ui.label(RichText::new(format!("{value}{unit}")).color(TEXT));
    });
    (changed, done)
}

enum Swatch {
    Off,
    Random,
    Palette(Color),
}

fn swatch(ui: &mut egui::Ui, kind: Swatch, on: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
    let painter = ui.painter();
    let center = rect.center();
    let radius = 12.0;
    let tip = match kind {
        Swatch::Off => {
            painter.circle(center, radius, BG, Stroke::new(1.5_f32, MUTED));
            painter.line_segment(
                [center + Vec2::new(-7.0, 7.0), center + Vec2::new(7.0, -7.0)],
                Stroke::new(1.5_f32, MUTED),
            );
            "Off"
        }
        Swatch::Random => {
            for i in 0..6 {
                let from = i as f32 / 6.0 * std::f32::consts::TAU;
                let to = from + std::f32::consts::TAU / 6.0;
                let hue: Color32 = egui::ecolor::Hsva::new(i as f32 / 6.0, 0.8, 1.0, 1.0).into();
                let points = (0..=6)
                    .map(|s| center + Vec2::angled(from + (to - from) * s as f32 / 6.0) * radius)
                    .chain(std::iter::once(center))
                    .collect();
                painter.add(egui::Shape::convex_polygon(points, hue, Stroke::NONE));
            }
            "Random"
        }
        Swatch::Palette(color) => {
            let Rgb(r, g, b) = color.preview();
            painter.circle(
                center,
                radius,
                Color32::from_rgb(r, g, b),
                Stroke::new(1.0_f32, STROKE),
            );
            color.name()
        }
    };
    if on {
        painter.circle_stroke(center, radius + 3.5, Stroke::new(2.0_f32, ACCENT));
    } else if response.hovered() {
        painter.circle_stroke(center, radius + 3.0, Stroke::new(1.0_f32, MUTED));
    }
    response.on_hover_text(tip)
}
