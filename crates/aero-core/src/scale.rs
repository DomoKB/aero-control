/// Firmware duty is 0–229, not 0–100 and not 0–255.
///
/// `0xE5` (229) is 100% on the Aero 15 EC map (`p37-ec-aero-15`). Control
/// Center converts percent with integer division (`duty * 229 / 100`), which
/// truncates. These functions round half up in both directions.
pub const DUTY_MAX: u8 = 229;

/// Fixed-mode floor from the scope (~35%). `percent_to_raw(35)` is 80, and
/// the same EC map lists `0x50` (80) as 35%.
pub const FIXED_FLOOR_PERCENT: u8 = 35;

pub const fn percent_to_raw(percent: u8) -> u8 {
    let percent = if percent > 100 { 100 } else { percent };
    let raw = (percent as u16 * DUTY_MAX as u16 + 50) / 100;
    raw as u8
}

pub const fn raw_to_percent(raw: u8) -> u8 {
    let raw = if raw > DUTY_MAX { DUTY_MAX } else { raw };
    let percent = (raw as u16 * 100 + (DUTY_MAX as u16 / 2)) / DUTY_MAX as u16;
    if percent > 100 {
        100
    } else {
        percent as u8
    }
}

/// Fixed duty cannot go below the floor unless `expert` is set.
pub fn clamp_fixed_duty(raw: u8, expert: bool) -> u8 {
    let floor = if expert {
        0
    } else {
        percent_to_raw(FIXED_FLOOR_PERCENT)
    };
    raw.clamp(floor, DUTY_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_rounds_half_up_instead_of_truncating() {
        // GCC floors 50% to 114 (`50 * 229 / 100`). Half up is 115.
        assert_eq!(50 * 229 / 100, 114);
        assert_eq!(percent_to_raw(50), 115);
        assert_eq!(percent_to_raw(0), 0);
        assert_eq!(percent_to_raw(100), 229);
        assert_eq!(percent_to_raw(35), 80);
        assert_eq!(percent_to_raw(80), 183);
        assert_eq!(percent_to_raw(1), 2);
    }

    #[test]
    fn raw_to_percent_rounds_half_up() {
        assert_eq!(raw_to_percent(0), 0);
        assert_eq!(raw_to_percent(229), 100);
        assert_eq!(raw_to_percent(115), 50);
        assert_eq!(raw_to_percent(114), 50);
        assert_eq!(raw_to_percent(113), 49);
        assert_eq!(raw_to_percent(80), 35);
        assert_eq!(raw_to_percent(183), 80);
        assert_eq!(raw_to_percent(2), 1);
    }

    #[test]
    fn fixed_floor_is_80_unless_expert() {
        assert_eq!(clamp_fixed_duty(10, false), 80);
        assert_eq!(clamp_fixed_duty(10, true), 10);
        assert_eq!(clamp_fixed_duty(229, false), 229);
        assert_eq!(clamp_fixed_duty(255, false), 229);
        assert_eq!(clamp_fixed_duty(80, false), 80);
    }
}
