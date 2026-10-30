//! Painted fan-curve editor: 0–100 °C by 0–100 %, stair steps like the firmware table.

use aero_core::{
    constrain_drag, duty_at, insert_point, raw_to_percent, remove_point, stair_polyline, CurvePoint,
};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Rounding, Sense, Shape, Stroke};

use super::{mix, ACCENT, BG, CARD_RAISED, MUTED, STROKE, TEXT};

const HIT_RADIUS: f32 = 14.0;

pub struct Marker {
    pub label: &'static str,
    pub temp_c: f32,
    pub color: Color32,
}

#[derive(Clone, Copy)]
struct Axes {
    plot: Rect,
}

impl Axes {
    fn to_screen(self, temp_c: f32, percent: f32) -> Pos2 {
        Pos2::new(
            self.plot.left() + self.plot.width() * temp_c / 100.0,
            self.plot.bottom() - self.plot.height() * percent / 100.0,
        )
    }

    fn snapped(self, pos: Pos2) -> (i32, i32) {
        let temp = (pos.x - self.plot.left()) / self.plot.width() * 100.0;
        let percent = (self.plot.bottom() - pos.y) / self.plot.height() * 100.0;
        (temp.round() as i32, percent.round() as i32)
    }

    fn point(self, point: &CurvePoint) -> Pos2 {
        self.to_screen(
            f32::from(point.temp_c),
            f32::from(raw_to_percent(point.duty_raw)),
        )
    }
}

fn hit(axes: Axes, points: &[CurvePoint], pos: Pos2) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .map(|(index, point)| (index, axes.point(point).distance(pos)))
        .filter(|(_, d)| *d <= HIT_RADIUS)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| index)
}

pub fn show(
    ui: &mut egui::Ui,
    points: &mut Vec<CurvePoint>,
    drag: &mut Option<usize>,
    markers: &[Marker],
    height: f32,
) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        Sense::click_and_drag(),
    );
    let axes = Axes {
        plot: Rect::from_min_max(
            rect.min + egui::vec2(40.0, 26.0),
            rect.max - egui::vec2(14.0, 40.0),
        ),
    };
    let pointer = response.interact_pointer_pos().or(response.hover_pos());
    if response.drag_started() {
        *drag = pointer.and_then(|pos| hit(axes, points, pos));
    }
    if let (Some(index), true, Some(pos)) = (*drag, response.dragged(), pointer) {
        let (temp, percent) = axes.snapped(pos);
        if let Some(point) = constrain_drag(points, index, temp, percent) {
            points[index] = point;
        }
    }
    if response.drag_stopped() {
        *drag = None;
    }
    if let Some(pos) = pointer {
        if response.double_clicked() && hit(axes, points, pos).is_none() {
            let (temp, percent) = axes.snapped(pos);
            let _ = insert_point(
                points,
                temp.clamp(30, 95) as u8,
                percent.clamp(0, 100) as u8,
            );
        }
        if response.secondary_clicked() {
            if let Some(index) = hit(axes, points, pos) {
                let _ = remove_point(points, index);
            }
        }
    }
    let focus = drag.or_else(|| response.hover_pos().and_then(|pos| hit(axes, points, pos)));
    if focus.is_some() {
        ui.ctx().set_cursor_icon(if drag.is_some() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        });
    }
    paint(ui.painter(), axes, points, markers, focus);
}

fn paint(
    painter: &egui::Painter,
    axes: Axes,
    points: &[CurvePoint],
    markers: &[Marker],
    focus: Option<usize>,
) {
    let plot = axes.plot;
    painter.rect_filled(plot.expand(2.0), Rounding::same(8.0), BG);
    let small = FontId::proportional(12.0);
    for step in 0..=10 {
        let v = step as f32 * 10.0;
        let x = axes.to_screen(v, 0.0).x;
        let y = axes.to_screen(0.0, v).y;
        let grid = Stroke::new(1.0_f32, mix(BG, STROKE, 0.55));
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            grid,
        );
        painter.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            grid,
        );
        painter.text(
            Pos2::new(x, plot.bottom() + 6.0),
            Align2::CENTER_TOP,
            format!("{}", v as u8),
            small.clone(),
            MUTED,
        );
        painter.text(
            Pos2::new(plot.left() - 8.0, y),
            Align2::RIGHT_CENTER,
            format!("{}", v as u8),
            small.clone(),
            MUTED,
        );
    }
    painter.text(
        Pos2::new(plot.center().x, plot.bottom() + 22.0),
        Align2::CENTER_TOP,
        "Temperature °C",
        small.clone(),
        TEXT,
    );
    painter.text(
        Pos2::new(plot.left() - 8.0, plot.top() - 14.0),
        Align2::RIGHT_CENTER,
        "Fan %",
        small.clone(),
        TEXT,
    );

    let percent: Vec<(f64, f64)> = points
        .iter()
        .map(|p| (f64::from(p.temp_c), f64::from(raw_to_percent(p.duty_raw))))
        .collect();
    let stair = stair_polyline(&percent, 100.0);
    let fill = mix(BG, ACCENT, 0.16);
    for pair in stair.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a.1 == b.1 && b.0 > a.0 {
            painter.rect_filled(
                Rect::from_two_pos(
                    axes.to_screen(a.0 as f32, a.1 as f32),
                    axes.to_screen(b.0 as f32, 0.0),
                )
                .expand2(egui::vec2(0.5, 0.0)),
                Rounding::ZERO,
                fill,
            );
        }
    }
    let line = stair
        .iter()
        .map(|(x, y)| axes.to_screen(*x as f32, *y as f32))
        .collect();
    painter.add(Shape::line(line, Stroke::new(2.5_f32, ACCENT)));

    for (row, marker) in markers.iter().enumerate() {
        let temp = marker.temp_c.clamp(0.0, 100.0);
        let top = axes.to_screen(temp, 100.0);
        let bottom = axes.to_screen(temp, 0.0);
        painter.add(Shape::dashed_line(
            &[top, bottom],
            Stroke::new(1.5_f32, marker.color),
            5.0,
            4.0,
        ));
        let target = duty_at(points, temp as u8).map(raw_to_percent);
        if let Some(target) = target {
            let at = axes.to_screen(temp, f32::from(target));
            painter.circle(at, 6.0, marker.color, Stroke::new(2.0_f32, BG));
        }
        let text = match target {
            Some(target) => format!("{} {temp:.0}°C · fans {target}%", marker.label),
            None => format!("{} {temp:.0}°C", marker.label),
        };
        let galley = painter.layout_no_wrap(text, FontId::proportional(12.5), marker.color);
        let at = plot.left_top() + egui::vec2(12.0, 12.0 + row as f32 * 26.0);
        let chip = Rect::from_min_size(at, galley.size() + egui::vec2(14.0, 6.0));
        painter.rect(
            chip,
            Rounding::same(6.0),
            CARD_RAISED,
            Stroke::new(1.0_f32, marker.color),
        );
        painter.galley(chip.min + egui::vec2(7.0, 3.0), galley, marker.color);
    }

    for (index, point) in points.iter().enumerate() {
        let at = axes.point(point);
        if focus == Some(index) {
            painter.circle(at, 9.0, ACCENT, Stroke::new(3.0_f32, TEXT));
        } else {
            painter.circle(at, 5.5, TEXT, Stroke::new(2.0_f32, BG));
        }
    }
    if let Some(point) = focus.and_then(|index| points.get(index)) {
        let at = axes.point(point);
        let text = format!("{} °C · {}%", point.temp_c, raw_to_percent(point.duty_raw));
        let above = if at.y - plot.top() > 40.0 {
            -26.0
        } else {
            26.0
        };
        label(painter, at + egui::vec2(0.0, above), &text, TEXT);
    }
}

fn label(painter: &egui::Painter, center: Pos2, text: &str, color: Color32) {
    let galley = painter.layout_no_wrap(text.to_string(), FontId::proportional(12.5), color);
    let size = galley.size() + egui::vec2(14.0, 6.0);
    let mut rect = Rect::from_center_size(center, size);
    let bounds = painter.clip_rect().shrink(2.0);
    rect = rect.translate(egui::vec2(
        (bounds.left() - rect.left()).max(0.0) + (bounds.right() - rect.right()).min(0.0),
        0.0,
    ));
    painter.rect(
        rect,
        Rounding::same(6.0),
        CARD_RAISED,
        Stroke::new(1.0_f32, mix(CARD_RAISED, color, 0.5)),
    );
    painter.galley(rect.center() - galley.size() / 2.0, galley, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axes() -> Axes {
        Axes {
            plot: Rect::from_min_size(Pos2::new(10.0, 20.0), egui::vec2(500.0, 200.0)),
        }
    }

    #[test]
    fn screen_positions_snap_back_to_whole_degrees_and_percent() {
        let a = axes();
        assert_eq!(a.snapped(a.to_screen(62.0, 50.0)), (62, 50));
        assert_eq!(a.snapped(a.to_screen(62.4, 49.6)), (62, 50));
        assert_eq!(a.to_screen(0.0, 0.0), Pos2::new(10.0, 220.0));
        assert_eq!(a.to_screen(100.0, 100.0), Pos2::new(510.0, 20.0));
    }

    #[test]
    fn hit_picks_the_closest_point_inside_the_radius() {
        let a = axes();
        let points = [
            CurvePoint {
                temp_c: 50,
                duty_raw: 69,
            },
            CurvePoint {
                temp_c: 53,
                duty_raw: 80,
            },
        ];
        let near_second = a.point(&points[1]) + egui::vec2(3.0, 0.0);
        assert_eq!(hit(a, &points, near_second), Some(1));
        let far = a.point(&points[1]) + egui::vec2(0.0, HIT_RADIUS + 1.0);
        assert_eq!(hit(a, &points, far), None);
    }
}
