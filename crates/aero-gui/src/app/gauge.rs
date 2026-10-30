//! Painted gauges, dials and the duty knob.

use std::f32::consts::PI;

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};

use super::{mix, ACCENT, AMBER, CARD_RAISED, HOT, MUTED, STROKE, TEXT, WARN};

const TEMP_START: f32 = 0.75 * PI;
const TEMP_SWEEP: f32 = 1.5 * PI;
const DIAL_START: f32 = 5.0 / 6.0 * PI;
const DIAL_SWEEP: f32 = 4.0 / 3.0 * PI;
pub const RPM_MAX: f32 = 6000.0;
const WARM_C: f32 = 60.0;
const HOT_C: f32 = 80.0;
const GOOD: Color32 = Color32::from_rgb(86, 200, 120);

pub fn temp_band(celsius: f32) -> Color32 {
    if celsius < WARM_C {
        GOOD
    } else if celsius < HOT_C {
        AMBER
    } else {
        HOT
    }
}

fn polar(center: Pos2, radius: f32, angle: f32) -> Pos2 {
    center + Vec2::angled(angle) * radius
}

fn arc(painter: &egui::Painter, center: Pos2, radius: f32, from: f32, to: f32, stroke: Stroke) {
    let steps = ((to - from).abs() * radius / 4.0).ceil().max(2.0) as usize;
    let points = (0..=steps)
        .map(|i| polar(center, radius, from + (to - from) * i as f32 / steps as f32))
        .collect();
    painter.add(Shape::line(points, stroke));
}

/// 0–100 °C ring with green, amber and red bands. `None` paints an empty ring.
pub fn temperature(painter: &egui::Painter, rect: Rect, celsius: Option<f32>) {
    let center = rect.center();
    let radius = rect.width().min(rect.height()) / 2.0 - 8.0;
    let width = (radius * 0.16).clamp(8.0, 16.0);
    let angle = |c: f32| TEMP_START + TEMP_SWEEP * (c / 100.0).clamp(0.0, 1.0);
    for (lo, hi, color) in [
        (0.0, WARM_C, GOOD),
        (WARM_C, HOT_C, AMBER),
        (HOT_C, 100.0, HOT),
    ] {
        let dim = mix(CARD_RAISED, color, 0.22);
        arc(
            painter,
            center,
            radius,
            angle(lo),
            angle(hi),
            Stroke::new(width, dim),
        );
    }
    let (text, color) = match celsius {
        Some(c) => {
            let color = temp_band(c);
            arc(
                painter,
                center,
                radius,
                TEMP_START,
                angle(c),
                Stroke::new(width, color),
            );
            painter.circle_filled(polar(center, radius, angle(c)), width * 0.62, TEXT);
            (format!("{c:.0}"), color)
        }
        None => ("—".to_string(), MUTED),
    };
    let size = (radius * 0.62).clamp(22.0, 54.0);
    painter.text(
        center - Vec2::new(0.0, size * 0.1),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(size),
        color,
    );
    painter.text(
        center + Vec2::new(0.0, size * 0.62),
        Align2::CENTER_CENTER,
        "°C",
        FontId::proportional(14.0),
        MUTED,
    );
    for c in [0.0, 100.0] {
        let pos = polar(center, radius + 2.0, angle(c)) + Vec2::new(0.0, width + 4.0);
        painter.text(
            pos,
            Align2::CENTER_CENTER,
            format!("{c:.0}"),
            FontId::proportional(11.0),
            MUTED,
        );
    }
}

/// Speedometer from 0 to [`RPM_MAX`] with a needle.
pub fn rpm(painter: &egui::Painter, rect: Rect, rpm: Option<f32>) {
    let center = rect.center() + Vec2::new(0.0, rect.height() * 0.06);
    let radius = rect.width().min(rect.height()) / 2.0 - 8.0;
    let angle = |r: f32| DIAL_START + DIAL_SWEEP * (r / RPM_MAX).clamp(0.0, 1.0);
    arc(
        painter,
        center,
        radius,
        DIAL_START,
        DIAL_START + DIAL_SWEEP,
        Stroke::new(6.0_f32, CARD_RAISED),
    );
    for step in 0..=24 {
        let r = step as f32 * 250.0;
        let major = step % 4 == 0;
        let outer = radius - 10.0;
        let inner = outer - if major { 12.0 } else { 6.0 };
        let stroke = Stroke::new(
            if major { 2.0_f32 } else { 1.0 },
            if major { TEXT } else { STROKE },
        );
        painter.line_segment(
            [
                polar(center, inner, angle(r)),
                polar(center, outer, angle(r)),
            ],
            stroke,
        );
        if major {
            painter.text(
                polar(center, inner - 12.0, angle(r)),
                Align2::CENTER_CENTER,
                if step == 0 {
                    "0".to_string()
                } else {
                    format!("{}k", step / 4)
                },
                FontId::proportional(12.0),
                MUTED,
            );
        }
    }
    let value = rpm.unwrap_or(0.0);
    if rpm.is_some() {
        arc(
            painter,
            center,
            radius,
            DIAL_START,
            angle(value),
            Stroke::new(6.0_f32, ACCENT),
        );
    }
    let tip = polar(center, radius - 18.0, angle(value));
    let tail = polar(center, radius * 0.14, angle(value) + PI);
    painter.line_segment([tail, tip], Stroke::new(3.0_f32, WARN));
    painter.circle_filled(center, 7.0, CARD_RAISED);
    painter.circle_stroke(center, 7.0, Stroke::new(2.0_f32, WARN));
    painter.text(
        center + Vec2::new(0.0, radius * 0.5),
        Align2::CENTER_CENTER,
        rpm.map_or_else(|| "—".to_string(), |r| format!("{r:.0}")),
        FontId::proportional((radius * 0.32).clamp(18.0, 32.0)),
        TEXT,
    );
    painter.text(
        center + Vec2::new(0.0, radius * 0.5 + 22.0),
        Align2::CENTER_CENTER,
        "RPM",
        FontId::proportional(11.0),
        MUTED,
    );
}

/// Drag-to-set percent knob. Values below `floor` are shown but cannot be picked.
pub fn knob(ui: &mut egui::Ui, value: &mut u8, floor: u8, size: f32) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click_and_drag());
    let center = rect.center();
    let radius = size / 2.0 - 12.0;
    let angle = |p: f32| TEMP_START + TEMP_SWEEP * (p / 100.0).clamp(0.0, 1.0);
    if response.dragged() || response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let offset = pointer - center;
            let mut a = offset.y.atan2(offset.x) - TEMP_START;
            a = a.rem_euclid(2.0 * PI);
            let percent = if a <= TEMP_SWEEP {
                a / TEMP_SWEEP * 100.0
            } else if a < TEMP_SWEEP + (2.0 * PI - TEMP_SWEEP) / 2.0 {
                100.0
            } else {
                0.0
            };
            let next = (percent.round() as u8).max(floor);
            if next != *value {
                *value = next;
                response.mark_changed();
            }
        }
    }
    if response.hovered() {
        let scroll = ui.input(|input| input.raw_scroll_delta.y);
        if scroll != 0.0 {
            let next =
                (i16::from(*value) + scroll.signum() as i16).clamp(i16::from(floor), 100) as u8;
            if next != *value {
                *value = next;
                response.mark_changed();
            }
        }
    }
    let painter = ui.painter_at(rect);
    let track = Stroke::new(10.0_f32, CARD_RAISED);
    arc(
        &painter,
        center,
        radius,
        TEMP_START,
        TEMP_START + TEMP_SWEEP,
        track,
    );
    if floor > 0 {
        arc(
            &painter,
            center,
            radius,
            TEMP_START,
            angle(f32::from(floor)),
            Stroke::new(10.0_f32, mix(CARD_RAISED, WARN, 0.3)),
        );
    }
    let v = f32::from(*value);
    arc(
        &painter,
        center,
        radius,
        TEMP_START,
        angle(v),
        Stroke::new(10.0_f32, ACCENT),
    );
    let face = radius - 16.0;
    let ring = if response.hovered() || response.dragged() {
        ACCENT
    } else {
        STROKE
    };
    painter.circle(
        center,
        face,
        mix(CARD_RAISED, Color32::BLACK, 0.15),
        Stroke::new(1.5_f32, ring),
    );
    painter.line_segment(
        [
            polar(center, face * 0.55, angle(v)),
            polar(center, face - 6.0, angle(v)),
        ],
        Stroke::new(3.0_f32, ACCENT),
    );
    painter.circle_filled(polar(center, radius, angle(v)), 8.0, TEXT);
    painter.text(
        center,
        Align2::CENTER_CENTER,
        format!("{}%", *value),
        FontId::proportional((face * 0.42).clamp(18.0, 34.0)),
        TEXT,
    );
    response.on_hover_text("Drag around the dial or scroll to set the duty.")
}
