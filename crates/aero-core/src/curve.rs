use serde::{Deserialize, Serialize};

use crate::scale::{percent_to_raw, raw_to_percent, DUTY_MAX};

pub const MIN_POINTS: usize = 2;
pub const MAX_POINTS: usize = 15;
pub const FAN_CURVE_SLOTS: usize = 15;
pub const HIGH_TEMP_C: u8 = 85;
pub const HIGH_TEMP_MIN_PERCENT: u8 = 50;
pub const LAST_MIN_PERCENT: u8 = 80;
pub const LAST_TEMP_MAX: u8 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurvePoint {
    pub temp_c: u8,
    pub duty_raw: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurveIssue {
    pub index: Option<usize>,
    pub message: String,
}

/// A curve that already passed [`issues`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanCurve {
    points: Vec<CurvePoint>,
}

impl FanCurve {
    pub fn try_new(points: Vec<CurvePoint>) -> Result<Self, Vec<CurveIssue>> {
        let found = issues(&points);
        if found.is_empty() {
            Ok(Self { points })
        } else {
            Err(found)
        }
    }

    pub fn points(&self) -> &[CurvePoint] {
        &self.points
    }

    /// Percent pairs for the editor. Percent is the rounded display value.
    pub fn percent_points(&self) -> Vec<(u8, u8)> {
        self.points
            .iter()
            .map(|point| (point.temp_c, raw_to_percent(point.duty_raw)))
            .collect()
    }
}

pub fn issues(points: &[CurvePoint]) -> Vec<CurveIssue> {
    let mut found = Vec::new();
    if !(MIN_POINTS..=MAX_POINTS).contains(&points.len()) {
        found.push(CurveIssue {
            index: None,
            message: format!(
                "a curve needs {MIN_POINTS} to {MAX_POINTS} points, got {}",
                points.len()
            ),
        });
    }
    for (index, point) in points.iter().enumerate() {
        if point.temp_c > 100 {
            found.push(CurveIssue {
                index: Some(index),
                message: format!("temperature {}°C is above 100°C", point.temp_c),
            });
        }
        if point.duty_raw > DUTY_MAX {
            found.push(CurveIssue {
                index: Some(index),
                message: format!(
                    "duty {} is above the firmware maximum {DUTY_MAX}",
                    point.duty_raw
                ),
            });
        }
        if point.temp_c == 0 && point.duty_raw == 0 {
            found.push(CurveIssue {
                index: Some(index),
                message: "a (0, 0) point terminates the firmware table and is not allowed anywhere in the curve".to_string(),
            });
        }
        if index > 0 {
            let prev = &points[index - 1];
            if point.temp_c <= prev.temp_c {
                found.push(CurveIssue {
                    index: Some(index),
                    message: format!(
                        "temperatures must be strictly increasing ({}°C then {}°C)",
                        prev.temp_c, point.temp_c
                    ),
                });
            }
            if point.duty_raw < prev.duty_raw {
                found.push(CurveIssue {
                    index: Some(index),
                    message: format!(
                        "duty must be non-decreasing (raw {} then {})",
                        prev.duty_raw, point.duty_raw
                    ),
                });
            }
        }
    }
    if let Some(last) = points.last() {
        let index = points.len() - 1;
        if last.temp_c > LAST_TEMP_MAX {
            found.push(CurveIssue {
                index: Some(index),
                message: format!("the last temperature must be ≤ {LAST_TEMP_MAX}°C"),
            });
        }
        if raw_to_percent(last.duty_raw) < LAST_MIN_PERCENT {
            found.push(CurveIssue {
                index: Some(index),
                message: format!("the last point must be ≥ {LAST_MIN_PERCENT}% duty"),
            });
        }
    }
    if !points.is_empty() {
        match duty_at(points, HIGH_TEMP_C) {
            None => found.push(CurveIssue {
                index: None,
                message: format!("the curve must cover {HIGH_TEMP_C}°C (a point at or below it)"),
            }),
            Some(duty) if raw_to_percent(duty) < HIGH_TEMP_MIN_PERCENT => found.push(CurveIssue {
                index: None,
                message: format!(
                    "duty at or above {HIGH_TEMP_C}°C must be ≥ {HIGH_TEMP_MIN_PERCENT}%"
                ),
            }),
            Some(_) => {}
        }
    }
    found
}

/// Stair-step duty: the last point at or below `temp_c` stays in force until
/// the next point. This is the behaviour the editor draws. It is not yet
/// confirmed against the AERO 17 YB.
pub fn duty_at(points: &[CurvePoint], temp_c: u8) -> Option<u8> {
    let mut current = None;
    for point in points {
        if point.temp_c <= temp_c {
            current = Some(point.duty_raw);
        } else {
            break;
        }
    }
    current
}

/// Fifteen firmware slots. User points come first. The first unused slot is
/// `(0, 0)`, which terminates the table, and every slot after that is `(0, 0)`
/// as well so a later garbage point cannot survive a partial old upload.
pub fn planned_slots(curve: &FanCurve) -> [(u8, u8); FAN_CURVE_SLOTS] {
    let mut slots = [(0, 0); FAN_CURVE_SLOTS];
    for (slot, point) in slots.iter_mut().zip(curve.points()) {
        *slot = (point.temp_c, point.duty_raw);
    }
    slots
}

/// Snap a drag to 1°C / 1% and keep the point between its neighbours.
pub fn constrain_drag(
    points: &[CurvePoint],
    index: usize,
    temp_c: i32,
    duty_percent: i32,
) -> Option<CurvePoint> {
    let current = *points.get(index)?;
    let prev = index.checked_sub(1).and_then(|i| points.get(i));
    let next = points.get(index + 1);

    let min_temp = prev.map(|point| i32::from(point.temp_c) + 1).unwrap_or(30);
    let max_temp = next.map(|point| i32::from(point.temp_c) - 1).unwrap_or(95);
    let temp = if min_temp > max_temp {
        current.temp_c
    } else {
        temp_c.clamp(min_temp, max_temp) as u8
    };

    let min_raw = prev.map(|point| u16::from(point.duty_raw)).unwrap_or(0);
    let max_raw = next
        .map(|point| u16::from(point.duty_raw))
        .unwrap_or(u16::from(DUTY_MAX));
    let percent = duty_percent.clamp(0, 100) as u8;
    // Keep exact imported or factory bytes when a drag changes only temperature.
    let requested = u16::from(if percent == raw_to_percent(current.duty_raw) {
        current.duty_raw
    } else {
        percent_to_raw(percent)
    });
    let duty_raw = if min_raw > max_raw {
        current.duty_raw
    } else {
        requested.clamp(min_raw, max_raw) as u8
    };
    Some(CurvePoint {
        temp_c: temp,
        duty_raw,
    })
}

pub fn insert_point(
    points: &mut Vec<CurvePoint>,
    temp_c: u8,
    duty_percent: u8,
) -> Result<(), &'static str> {
    if points.len() >= MAX_POINTS {
        return Err("a curve holds at most 15 points");
    }
    if points.iter().any(|point| point.temp_c == temp_c) {
        return Err("a point already exists at that temperature");
    }
    let insert_at = points
        .iter()
        .position(|point| point.temp_c > temp_c)
        .unwrap_or(points.len());
    let min_raw = insert_at
        .checked_sub(1)
        .map(|index| points[index].duty_raw)
        .unwrap_or(0);
    let max_raw = points
        .get(insert_at)
        .map(|point| point.duty_raw)
        .unwrap_or(DUTY_MAX);
    let duty_raw = if min_raw > max_raw {
        min_raw
    } else {
        percent_to_raw(duty_percent).clamp(min_raw, max_raw)
    };
    points.insert(insert_at, CurvePoint { temp_c, duty_raw });
    Ok(())
}

pub fn remove_point(points: &mut Vec<CurvePoint>, index: usize) -> Result<(), &'static str> {
    if points.len() <= MIN_POINTS {
        return Err("a curve needs at least 2 points");
    }
    if index >= points.len() {
        return Err("no point at that index");
    }
    points.remove(index);
    Ok(())
}

/// Horizontal-then-vertical segments. `x_end` extends the last duty to the
/// right edge of the plot.
pub fn stair_polyline(points: &[(f64, f64)], x_end: f64) -> Vec<(f64, f64)> {
    if points.is_empty() {
        return Vec::new();
    }
    let mut line = vec![points[0]];
    for pair in points.windows(2) {
        line.push((pair[1].0, pair[0].1));
        line.push(pair[1]);
    }
    if let Some(&(x, y)) = points.last() {
        if x_end > x {
            line.push((x_end, y));
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(temp: u8, percent: u8) -> CurvePoint {
        CurvePoint {
            temp_c: temp,
            duty_raw: percent_to_raw(percent),
        }
    }

    fn valid_two() -> FanCurve {
        FanCurve::try_new(vec![point(85, 50), point(90, 80)]).unwrap()
    }

    #[test]
    fn accepts_a_two_point_curve_that_meets_the_floors() {
        let curve = valid_two();
        assert_eq!(curve.points().len(), 2);
    }

    #[test]
    fn rejects_one_point_and_sixteen_points() {
        assert!(issues(&[point(90, 100)])
            .iter()
            .any(|issue| issue.message.contains("2 to 15")));
        let mut many: Vec<_> = (0..16).map(|index| point(40 + index, 80)).collect();
        many.last_mut().unwrap().temp_c = 90;
        assert!(issues(&many)
            .iter()
            .any(|issue| issue.message.contains("2 to 15")));
    }

    #[test]
    fn rejects_a_terminator_anywhere_including_the_middle() {
        let points = vec![
            point(40, 40),
            CurvePoint {
                temp_c: 0,
                duty_raw: 0,
            },
            point(90, 100),
        ];
        let found = issues(&points);
        assert!(found
            .iter()
            .any(|issue| issue.index == Some(1) && issue.message.contains("(0, 0)")));
    }

    #[test]
    fn rejects_decreasing_duty_and_flat_temperatures() {
        let flat = issues(&[point(70, 80), point(70, 90)]);
        assert!(flat
            .iter()
            .any(|issue| issue.message.contains("strictly increasing")));
        let down = issues(&[
            CurvePoint {
                temp_c: 40,
                duty_raw: 200,
            },
            CurvePoint {
                temp_c: 90,
                duty_raw: 183,
            },
        ]);
        assert!(down
            .iter()
            .any(|issue| issue.message.contains("non-decreasing")));
    }

    #[test]
    fn enforces_the_85c_floor_and_the_last_point() {
        let early = issues(&[point(40, 30), point(90, 100)]);
        assert!(early.iter().any(|issue| issue.message.contains("85")));
        let hot = issues(&[point(85, 80), point(91, 100)]);
        assert!(hot.iter().any(|issue| issue.message.contains("90")));
        let weak = issues(&[point(85, 50), point(90, 70)]);
        assert!(weak.iter().any(|issue| issue.message.contains("80%")));
    }

    #[test]
    fn fifteen_slots_pad_with_a_terminator_and_keep_a_full_table() {
        let curve = valid_two();
        let slots = planned_slots(&curve);
        assert_eq!(slots[0], (85, percent_to_raw(50)));
        assert_eq!(slots[1], (90, percent_to_raw(80)));
        assert!(slots[2..].iter().all(|slot| *slot == (0, 0)));

        let mut full = Vec::new();
        for index in 0..13u8 {
            full.push(point(40 + index * 3, 50));
        }
        full.push(point(85, 80));
        full.push(point(90, 100));
        let curve = FanCurve::try_new(full).unwrap();
        let slots = planned_slots(&curve);
        assert!(slots.iter().all(|slot| *slot != (0, 0)));
        assert_eq!(slots[14].0, 90);
    }

    #[test]
    fn drag_keeps_exact_raw_duty_when_percent_is_unchanged() {
        let points = vec![
            point(40, 25),
            CurvePoint {
                temp_c: 50,
                duty_raw: 68,
            },
            point(90, 100),
        ];
        let moved = constrain_drag(&points, 1, 51, 30).unwrap();
        assert_eq!(
            moved,
            CurvePoint {
                temp_c: 51,
                duty_raw: 68
            }
        );
    }

    #[test]
    fn drag_stays_between_neighbours_and_snaps() {
        let points = vec![point(40, 35), point(60, 50), point(90, 100)];
        let moved = constrain_drag(&points, 1, 100, 10).unwrap();
        assert_eq!(moved.temp_c, 89);
        assert!(moved.duty_raw >= points[0].duty_raw);
        assert!(moved.duty_raw <= points[2].duty_raw);
        let high = constrain_drag(&points, 1, 61, 100).unwrap();
        assert_eq!(high.temp_c, 61);
        assert!(high.duty_raw <= points[2].duty_raw);
    }

    #[test]
    fn insert_and_remove_respect_the_point_limits() {
        let mut points = vec![point(40, 40), point(90, 100)];
        insert_point(&mut points, 70, 70).unwrap();
        assert_eq!(points[1].temp_c, 70);
        assert!(remove_point(&mut points, 1).is_ok());
        assert!(remove_point(&mut points, 0).is_err());
        assert!(insert_point(&mut points, 40, 50).is_err());
    }

    #[test]
    fn stair_step_holds_duty_until_the_next_temperature() {
        let line = stair_polyline(&[(40.0, 30.0), (60.0, 50.0)], 100.0);
        assert_eq!(
            line,
            vec![(40.0, 30.0), (60.0, 30.0), (60.0, 50.0), (100.0, 50.0)]
        );
    }
}
