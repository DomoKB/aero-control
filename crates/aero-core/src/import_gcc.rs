//! Read-only import of Control Center's `CustomFanTable`.
//!
//! The registry value's layout is not documented. This parser accepts text,
//! UTF-16 text, interleaved byte pairs, and a 15-temperature + 15-duty block.
//! It does not write the registry. A `(0, 0)` pair is treated as the firmware
//! terminator: later points are returned separately instead of being stitched
//! back in.

use crate::curve::CurvePoint;
use crate::scale::{percent_to_raw, DUTY_MAX};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GccImport {
    pub points: Vec<CurvePoint>,
    pub dropped_after_terminator: Vec<CurvePoint>,
    pub warnings: Vec<String>,
    /// `text`, `utf16`, `interleaved`, `split-15`, or `header`.
    pub encoding: &'static str,
}

pub fn parse_custom_fan_table_text(text: &str) -> GccImport {
    let mut warnings = Vec::new();
    let mut explicit_raw = None;
    let mut numbers: Vec<(u8, u8)> = Vec::new();
    for raw_line in text.split([';', '\n', '|']) {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if numbers.is_empty() && explicit_raw.is_none() {
            if line.eq_ignore_ascii_case("raw") {
                explicit_raw = Some(true);
                continue;
            }
            if line.eq_ignore_ascii_case("percent") {
                explicit_raw = Some(false);
                continue;
            }
        }
        let parts: Vec<&str> = line
            .split(|ch: char| ch == ',' || ch.is_whitespace())
            .filter(|part| !part.is_empty())
            .collect();
        if parts.len() != 2 {
            warnings.push(format!("skipped \"{line}\": expected temperature, duty"));
            continue;
        }
        let Ok(temp) = parts[0].parse::<u8>() else {
            warnings.push(format!("skipped \"{line}\": temperature is not a byte"));
            continue;
        };
        let Ok(duty) = parts[1].parse::<u8>() else {
            warnings.push(format!("skipped \"{line}\": duty is not a byte"));
            continue;
        };
        numbers.push((temp, duty));
    }
    let duties_are_raw = match explicit_raw {
        Some(raw) => raw,
        None => {
            let raw = numbers.iter().any(|(_, duty)| *duty > 100);
            if !raw {
                warnings.push(
                    "all duty values are ≤ 100; interpreted as percent. This layout is provisional."
                        .to_string(),
                );
            }
            raw
        }
    };
    let encoding = if explicit_raw.is_some() {
        "header"
    } else {
        "text"
    };
    finish(numbers, duties_are_raw, encoding, warnings)
}

pub fn parse_custom_fan_table_auto(bytes: &[u8]) -> GccImport {
    if let Some(text) = decode_text(bytes) {
        if text.contains(',') || text.contains(';') || text.contains('\n') {
            let mut parsed = parse_custom_fan_table_text(&text);
            if bytes_look_like_utf16(bytes) {
                parsed.encoding = "utf16";
            }
            return parsed;
        }
    }
    if bytes.len() % 2 != 0 || bytes.is_empty() {
        return GccImport {
            points: Vec::new(),
            dropped_after_terminator: Vec::new(),
            warnings: vec!["binary CustomFanTable length is not a list of pairs".to_string()],
            encoding: "interleaved",
        };
    }
    let interleaved: Vec<(u8, u8)> = bytes.chunks(2).map(|pair| (pair[0], pair[1])).collect();
    if bytes.len() == 30 {
        let split: Vec<(u8, u8)> = (0..15)
            .map(|index| (bytes[index], bytes[15 + index]))
            .collect();
        let left = score(&interleaved);
        let right = score(&split);
        if right < left {
            let mut parsed = finish(split, true, "split-15", Vec::new());
            parsed.warnings.push(
                "30-byte table interpreted as 15 temperatures followed by 15 duties. This layout is provisional."
                    .to_string(),
            );
            return parsed;
        }
    }
    let mut warnings = Vec::new();
    let duties_are_raw = interleaved.iter().any(|(_, duty)| *duty > 100);
    if !duties_are_raw {
        warnings.push(
            "interleaved duties are all ≤ 100; interpreted as percent. This layout is provisional."
                .to_string(),
        );
    }
    finish(interleaved, duties_are_raw, "interleaved", warnings)
}

fn decode_text(bytes: &[u8]) -> Option<String> {
    if bytes_look_like_utf16(bytes) {
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair.get(1).copied().unwrap_or(0)]))
            .take_while(|unit| *unit != 0)
            .collect();
        return String::from_utf16(&units).ok();
    }
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end]).ok().map(str::to_string)
}

fn bytes_look_like_utf16(bytes: &[u8]) -> bool {
    bytes.len() >= 4
        && bytes.len() % 2 == 0
        && bytes.chunks(2).filter(|pair| pair[1] == 0).count() * 2 >= bytes.len() / 2
}

fn score(pairs: &[(u8, u8)]) -> usize {
    let mut problems = 0;
    let mut last_temp = None;
    for (temp, duty) in pairs {
        if *duty > DUTY_MAX {
            problems += 2;
        }
        if let Some(prev) = last_temp {
            if *temp <= prev {
                problems += 1;
            }
        }
        last_temp = Some(*temp);
        if *temp == 0 && *duty == 0 {
            break;
        }
    }
    problems
}

fn finish(
    numbers: Vec<(u8, u8)>,
    duties_are_raw: bool,
    encoding: &'static str,
    mut warnings: Vec<String>,
) -> GccImport {
    let mut points = Vec::new();
    let mut dropped = Vec::new();
    let mut terminated = false;
    for (temp, duty) in numbers {
        if duty > DUTY_MAX {
            warnings.push(format!("skipped {temp},{duty}: duty is above {DUTY_MAX}"));
            continue;
        }
        let point = CurvePoint {
            temp_c: temp,
            duty_raw: if duties_are_raw {
                duty
            } else {
                percent_to_raw(duty.min(100))
            },
        };
        if temp == 0 && duty == 0 {
            terminated = true;
            warnings.push(
                "a (0, 0) pair terminates the firmware table; later points were not imported"
                    .to_string(),
            );
            continue;
        }
        if terminated {
            dropped.push(point);
        } else {
            points.push(point);
        }
    }
    GccImport {
        points,
        dropped_after_terminator: dropped,
        warnings,
        encoding,
    }
}

#[cfg(windows)]
pub fn read_gcc_custom_fan_table() -> Result<GccImport, String> {
    let bytes = crate::registry::read_custom_fan_table()?;
    Ok(parse_custom_fan_table_auto(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::percent_to_raw;

    #[test]
    fn text_percent_curve_and_a_mid_terminator_keep_the_tail_visible() {
        let parsed = parse_custom_fan_table_text("45,35;60,45;0,0;85,80;90,100");
        assert_eq!(parsed.points.len(), 2);
        assert_eq!(parsed.points[0].duty_raw, percent_to_raw(35));
        assert_eq!(parsed.dropped_after_terminator.len(), 2);
        assert!(parsed
            .warnings
            .iter()
            .any(|warning| warning.contains("(0, 0)")));
    }

    #[test]
    fn raw_header_is_not_treated_as_percent() {
        let parsed = parse_custom_fan_table_text("raw\n85,114\n90,183");
        assert_eq!(parsed.encoding, "header");
        assert_eq!(parsed.points[0].duty_raw, 114);
        assert_eq!(parsed.points[1].duty_raw, 183);
    }

    #[test]
    fn interleaved_bytes_above_100_are_raw() {
        let bytes = [45, 80, 90, 229];
        let parsed = parse_custom_fan_table_auto(&bytes);
        assert_eq!(parsed.encoding, "interleaved");
        assert_eq!(parsed.points[1].duty_raw, 229);
    }

    #[test]
    fn utf16_registry_text_is_decoded() {
        let text = "45,35;60,45;75,60;85,80;90,100";
        let mut bytes = Vec::new();
        for unit in text.encode_utf16() {
            bytes.extend(unit.to_le_bytes());
        }
        let parsed = parse_custom_fan_table_auto(&bytes);
        assert_eq!(parsed.encoding, "utf16");
        assert_eq!(parsed.points.len(), 5);
        assert_eq!(parsed.points[0].duty_raw, percent_to_raw(35));
    }
}
