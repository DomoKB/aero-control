//! Painted keyboard preview. Colours approximate the effect; the keyboard animates it.

use aero_kbd::{Color, Effect, LightSettings, Rgb};
use eframe::egui::{self, ecolor::Hsva, Color32, Rect, Rounding};

use super::{mix, BG};

const KEY_OFF: Color32 = Color32::from_rgb(30, 36, 46);
const ROWS: [&[f32]; 5] = [
    &[1.0; 15],
    &[
        1.5, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.5,
    ],
    &[
        1.75, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 2.25,
    ],
    &[2.25, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 2.75],
    &[1.25, 1.25, 1.25, 6.25, 1.25, 1.25, 1.25, 1.25],
];

fn rainbow(t: f32) -> Color32 {
    Hsva::new(t.rem_euclid(1.0), 0.8, 1.0, 1.0).into()
}

/// Colour of the key centred at (`x`, `y`), both 0..1 across the keyboard.
pub fn key_color(settings: &LightSettings, custom: [u8; 3], x: f32, y: f32) -> Color32 {
    if settings.brightness == 0 {
        return KEY_OFF;
    }
    let level = f32::from(settings.brightness) / f32::from(aero_kbd::proto::BRIGHTNESS_MAX);
    let base = match (settings.effect, settings.color) {
        (Effect::Custom(_), _) => Color32::from_rgb(custom[0], custom[1], custom[2]),
        (_, Color::Random) => rainbow(x * 0.85 + y * 0.1),
        (_, color) => {
            let Rgb(r, g, b) = color.preview();
            Color32::from_rgb(r, g, b)
        }
    };
    let shape = match settings.effect {
        Effect::Static | Effect::Custom(_) => 1.0,
        Effect::Breathing => 0.35 + 0.65 * (1.0 - y),
        Effect::FadeOnKeypress | Effect::FlashOnKeypress | Effect::Ripple | Effect::Raindrop => {
            let hot = ((x * 15.0) as u32 * 7 + (y * 5.0) as u32 * 5) % 6 == 0;
            if hot {
                1.0
            } else {
                0.12
            }
        }
        _ => 0.45 + 0.55 * (x * std::f32::consts::TAU).sin().abs(),
    };
    mix(KEY_OFF, base, (level * shape).clamp(0.0, 1.0))
}

pub fn paint(painter: &egui::Painter, rect: Rect, settings: &LightSettings, custom: [u8; 3]) {
    painter.rect_filled(rect, Rounding::same(12.0), BG);
    let inner = rect.shrink(10.0);
    let gap = (inner.width() / 160.0).clamp(2.0, 4.0);
    let row_h = (inner.height() - gap * (ROWS.len() as f32 - 1.0)) / ROWS.len() as f32;
    for (row, widths) in ROWS.iter().enumerate() {
        let units: f32 = widths.iter().sum();
        let unit = (inner.width() - gap * (widths.len() as f32 - 1.0)) / units;
        let top = inner.top() + row as f32 * (row_h + gap);
        let mut left = inner.left();
        for width in widths.iter() {
            let key = Rect::from_min_size(egui::pos2(left, top), egui::vec2(width * unit, row_h));
            let x = (key.center().x - inner.left()) / inner.width();
            let y = row as f32 / (ROWS.len() - 1) as f32;
            painter.rect_filled(key, Rounding::same(3.0), key_color(settings, custom, x, y));
            left += width * unit + gap;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(effect: Effect, color: Color, brightness: u8) -> LightSettings {
        LightSettings {
            effect,
            color,
            brightness,
            ..LightSettings::default()
        }
    }

    #[test]
    fn off_keys_are_dark_whatever_the_effect() {
        let settings = with(Effect::Wave, Color::Random, 0);
        assert_eq!(key_color(&settings, [0; 3], 0.3, 0.5), KEY_OFF);
    }

    #[test]
    fn static_colour_fills_every_key_at_full_brightness() {
        let settings = with(Effect::Static, Color::Red, 50);
        let red = Color32::from_rgb(255, 0, 0);
        assert_eq!(key_color(&settings, [0; 3], 0.1, 0.0), red);
        assert_eq!(key_color(&settings, [0; 3], 0.9, 1.0), red);
    }

    #[test]
    fn random_colour_changes_across_the_keyboard() {
        let settings = with(Effect::Static, Color::Random, 50);
        assert_ne!(
            key_color(&settings, [0; 3], 0.1, 0.0),
            key_color(&settings, [0; 3], 0.6, 0.0)
        );
    }

    #[test]
    fn custom_slots_preview_the_fill_colour() {
        let settings = with(Effect::Custom(2), Color::Red, 50);
        assert_eq!(
            key_color(&settings, [0, 0, 255], 0.5, 0.5),
            Color32::from_rgb(0, 0, 255)
        );
    }
}
