//! Report layouts and Control Center's option model for the "Ite" keyboard.

use std::fmt;

pub const REPORT_LEN: usize = crate::FEATURE_LEN;
pub type Report = [u8; REPORT_LEN];

/// Command 0x08: set the running effect.
pub const CMD_SET_EFFECT: u8 = 0x08;
/// Command 0x88: request the running effect (answer via GetFeature).
pub const CMD_GET_EFFECT: u8 = 0x88;
/// Command 0x80: request the firmware version.
pub const CMD_GET_VERSION: u8 = 0x80;
/// Command 0x12: start a Custom-slot picture upload (8 x 64 bytes follow).
pub const CMD_SET_PICTURE: u8 = 0x12;
/// Command 0x92: start a Custom-slot picture download (8 x 64 bytes follow).
pub const CMD_GET_PICTURE: u8 = 0x92;

/// Per-key picture: 128 entries of `[0, r, g, b]`.
pub const MATRIX_LEN: usize = 512;
pub const MATRIX_CHUNK: usize = 64;
pub const MATRIX_ENTRIES: usize = MATRIX_LEN / 4;

pub const BRIGHTNESS_MAX: u8 = 50;
pub const SPEED_FASTEST: u8 = 1;
pub const SPEED_SLOWEST: u8 = 10;

/// `255 - sum(bytes 1..=7)` (wrapping).
pub fn checksum(report: &Report) -> u8 {
    let sum = report[1..8].iter().fold(0u8, |acc, b| acc.wrapping_add(*b));
    255u8.wrapping_sub(sum)
}

fn finish(mut report: Report) -> Report {
    report[8] = checksum(&report);
    report
}

/// Every lighting effect Control Center lists for this keyboard, in its combo order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Effect {
    Static,
    Breathing,
    Wave,
    FadeOnKeypress,
    Marquee,
    Ripple,
    FlashOnKeypress,
    Neon,
    RainbowMarquee,
    Raindrop,
    CircleMarquee,
    Hedge,
    Rotate,
    Custom(u8),
}

/// Which direction buttons Control Center shows for an effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectionKind {
    None,
    /// L→R, R→L, Down→Up, Up→Down (Wave).
    Linear,
    /// Clockwise, anticlockwise (Rotate).
    Rotate,
}

impl Effect {
    pub const ALL: [Effect; 18] = [
        Effect::Static,
        Effect::Breathing,
        Effect::Wave,
        Effect::FadeOnKeypress,
        Effect::Marquee,
        Effect::Ripple,
        Effect::FlashOnKeypress,
        Effect::Neon,
        Effect::RainbowMarquee,
        Effect::Raindrop,
        Effect::CircleMarquee,
        Effect::Hedge,
        Effect::Rotate,
        Effect::Custom(1),
        Effect::Custom(2),
        Effect::Custom(3),
        Effect::Custom(4),
        Effect::Custom(5),
    ];

    /// Byte 3 of command 0x08.
    pub fn code(self) -> u8 {
        match self {
            Effect::Static => 1,
            Effect::Breathing => 2,
            Effect::Wave => 3,
            Effect::FadeOnKeypress => 4,
            Effect::Marquee => 5,
            Effect::Ripple => 6,
            Effect::FlashOnKeypress => 7,
            Effect::Neon => 8,
            Effect::RainbowMarquee => 9,
            Effect::Raindrop => 10,
            Effect::CircleMarquee => 11,
            Effect::Hedge => 12,
            Effect::Rotate => 13,
            Effect::Custom(n) => 50 + n,
        }
    }

    pub fn from_code(code: u8) -> Option<Effect> {
        Effect::ALL.into_iter().find(|e| e.code() == code)
    }

    /// Control Center's on-screen label.
    pub fn gcc_name(self) -> &'static str {
        match self {
            Effect::Static => "STATIC",
            Effect::Breathing => "BREATHING",
            Effect::Wave => "WAVE",
            Effect::FadeOnKeypress => "FADE ON KEYPRESS",
            Effect::Marquee => "MARQUEE",
            Effect::Ripple => "RIPPLE",
            Effect::FlashOnKeypress => "FLASH ON KEYPRESS",
            Effect::Neon => "NEON",
            Effect::RainbowMarquee => "RAINBOW MARQUEE",
            Effect::Raindrop => "RAINDROP",
            Effect::CircleMarquee => "CIRCLE MARQUEE",
            Effect::Hedge => "HEDGE",
            Effect::Rotate => "ROTATE",
            Effect::Custom(1) => "CUSTOM 1",
            Effect::Custom(2) => "CUSTOM 2",
            Effect::Custom(3) => "CUSTOM 3",
            Effect::Custom(4) => "CUSTOM 4",
            Effect::Custom(_) => "CUSTOM 5",
        }
    }

    /// Control Center's registry / profile key ("static", "fadeonkeypress", ...).
    pub fn key(self) -> &'static str {
        match self {
            Effect::Static => "static",
            Effect::Breathing => "breathing",
            Effect::Wave => "wave",
            Effect::FadeOnKeypress => "fadeonkeypress",
            Effect::Marquee => "marquee",
            Effect::Ripple => "ripple",
            Effect::FlashOnKeypress => "flashonkeypress",
            Effect::Neon => "neon",
            Effect::RainbowMarquee => "rainbowmarquee",
            Effect::Raindrop => "raindrop",
            Effect::CircleMarquee => "circlemarquee",
            Effect::Hedge => "hedge",
            Effect::Rotate => "rotate",
            Effect::Custom(1) => "custom1",
            Effect::Custom(2) => "custom2",
            Effect::Custom(3) => "custom3",
            Effect::Custom(4) => "custom4",
            Effect::Custom(_) => "custom5",
        }
    }

    pub fn from_key(key: &str) -> Option<Effect> {
        let key = key.trim().to_ascii_lowercase();
        Effect::ALL.into_iter().find(|e| e.key() == key)
    }

    /// Wave, Neon, Rainbow marquee and Raindrop hide the palette and run Random.
    pub fn forces_random(self) -> bool {
        matches!(
            self,
            Effect::Wave | Effect::Neon | Effect::RainbowMarquee | Effect::Raindrop
        )
    }

    pub fn shows_palette(self) -> bool {
        !self.forces_random() && !self.is_custom()
    }

    /// Speed is hidden only for Static with a fixed colour, and for Custom slots.
    pub fn shows_speed(self, color: Color) -> bool {
        match self {
            Effect::Static => color == Color::Random,
            Effect::Custom(_) => false,
            _ => true,
        }
    }

    pub fn direction_kind(self) -> DirectionKind {
        match self {
            Effect::Wave => DirectionKind::Linear,
            Effect::Rotate => DirectionKind::Rotate,
            _ => DirectionKind::None,
        }
    }

    pub fn is_custom(self) -> bool {
        matches!(self, Effect::Custom(_))
    }

    /// Custom slot index used by commands 0x12 / 0x92 (0..=4).
    pub fn custom_slot(self) -> Option<u8> {
        match self {
            Effect::Custom(n) => Some(n - 1),
            _ => None,
        }
    }
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.gcc_name())
    }
}

/// Control Center's palette (byte 6 of command 0x08).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Orange,
    Purple,
    White,
    Random,
}

impl Color {
    /// The swatches Control Center shows, in its on-screen order.
    pub const PALETTE: [Color; 8] = [
        Color::Red,
        Color::Orange,
        Color::Yellow,
        Color::Green,
        Color::Blue,
        Color::Purple,
        Color::Black,
        Color::White,
    ];

    pub fn code(self) -> u8 {
        match self {
            Color::Black => 0,
            Color::Red => 1,
            Color::Green => 2,
            Color::Yellow => 3,
            Color::Blue => 4,
            Color::Orange => 5,
            Color::Purple => 6,
            Color::White => 7,
            Color::Random => 8,
        }
    }

    pub fn from_code(code: u8) -> Option<Color> {
        [
            Color::Black,
            Color::Red,
            Color::Green,
            Color::Yellow,
            Color::Blue,
            Color::Orange,
            Color::Purple,
            Color::White,
            Color::Random,
        ]
        .into_iter()
        .find(|c| c.code() == code)
    }

    pub fn name(self) -> &'static str {
        match self {
            Color::Black => "Black",
            Color::Red => "Red",
            Color::Green => "Green",
            Color::Yellow => "Yellow",
            Color::Blue => "Blue",
            Color::Orange => "Orange",
            Color::Purple => "Purple",
            Color::White => "White",
            Color::Random => "Random",
        }
    }

    /// Swatch colour for the UI (preview only; the keyboard gets the index).
    pub fn preview(self) -> Rgb {
        match self {
            Color::Black => Rgb(0, 0, 0),
            Color::Red => Rgb(255, 0, 0),
            Color::Green => Rgb(124, 252, 76),
            Color::Yellow => Rgb(255, 255, 0),
            Color::Blue => Rgb(65, 105, 225),
            Color::Orange => Rgb(255, 119, 0),
            Color::Purple => Rgb(148, 0, 211),
            Color::White => Rgb(255, 255, 255),
            Color::Random => Rgb(160, 160, 160),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    LeftToRight,
    RightToLeft,
    DownToUp,
    UpToDown,
}

impl Direction {
    pub const LINEAR: [Direction; 4] = [
        Direction::LeftToRight,
        Direction::RightToLeft,
        Direction::DownToUp,
        Direction::UpToDown,
    ];

    pub fn code(self) -> u8 {
        match self {
            Direction::LeftToRight => 1,
            Direction::RightToLeft => 2,
            Direction::DownToUp => 3,
            Direction::UpToDown => 4,
        }
    }

    pub fn from_code(code: u8) -> Option<Direction> {
        Direction::LINEAR.into_iter().find(|d| d.code() == code)
    }

    /// Label for the effect's direction kind (Rotate reuses codes 1/2 as CW/ACW).
    pub fn label(self, kind: DirectionKind) -> &'static str {
        match (kind, self) {
            (DirectionKind::Rotate, Direction::LeftToRight) => "Clockwise",
            (DirectionKind::Rotate, _) => "Anticlockwise",
            (_, Direction::LeftToRight) => "Left → Right",
            (_, Direction::RightToLeft) => "Right → Left",
            (_, Direction::DownToUp) => "Down → Up",
            (_, Direction::UpToDown) => "Up → Down",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    Brightness(u8),
    Speed(u8),
    Direction(Effect, u8),
    Color(Effect, Color),
    CustomSlot(u8),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Brightness(b) => write!(f, "brightness {b} is outside 0..=50"),
            SettingsError::Speed(s) => write!(f, "speed {s} is outside 1..=10"),
            SettingsError::Direction(e, d) => write!(f, "direction {d} is not offered for {e}"),
            SettingsError::Color(e, c) => write!(f, "{e} does not take colour {}", c.name()),
            SettingsError::CustomSlot(n) => write!(f, "custom slot {n} is outside 1..=5"),
        }
    }
}

impl std::error::Error for SettingsError {}

/// The five values carried by commands 0x08 / 0x88.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightSettings {
    pub effect: Effect,
    pub color: Color,
    /// 0..=50; 0 is Control Center's "Off".
    pub brightness: u8,
    /// 1 (fastest) ..= 10 (slowest).
    pub speed: u8,
    pub direction: Direction,
}

impl Default for LightSettings {
    /// Control Center's controller start state: Static, Red, speed 1, brightness 50, L→R.
    fn default() -> Self {
        LightSettings {
            effect: Effect::Static,
            color: Color::Red,
            brightness: BRIGHTNESS_MAX,
            speed: 1,
            direction: Direction::LeftToRight,
        }
    }
}

impl LightSettings {
    /// What Control Center loads for an effect with no saved entry: 50 / 5 / Red / 1,
    /// with Random forced on the palette-less effects.
    pub fn effect_default(effect: Effect) -> Self {
        LightSettings {
            effect,
            color: Color::Red,
            brightness: BRIGHTNESS_MAX,
            speed: 5,
            direction: Direction::LeftToRight,
        }
        .normalized()
    }

    /// Apply Control Center's per-effect rules: palette-less effects run Random,
    /// Rotate only knows CW/ACW.
    pub fn normalized(mut self) -> Self {
        if self.effect.forces_random() {
            self.color = Color::Random;
        }
        if self.effect.direction_kind() == DirectionKind::Rotate
            && !matches!(
                self.direction,
                Direction::LeftToRight | Direction::RightToLeft
            )
        {
            self.direction = Direction::LeftToRight;
        }
        self
    }

    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.brightness > BRIGHTNESS_MAX {
            return Err(SettingsError::Brightness(self.brightness));
        }
        if !(SPEED_FASTEST..=SPEED_SLOWEST).contains(&self.speed) {
            return Err(SettingsError::Speed(self.speed));
        }
        if let Effect::Custom(n) = self.effect {
            if !(1..=5).contains(&n) {
                return Err(SettingsError::CustomSlot(n));
            }
        }
        if self.effect.forces_random() && self.color != Color::Random {
            return Err(SettingsError::Color(self.effect, self.color));
        }
        if self.effect.direction_kind() == DirectionKind::Rotate
            && !matches!(
                self.direction,
                Direction::LeftToRight | Direction::RightToLeft
            )
        {
            return Err(SettingsError::Direction(self.effect, self.direction.code()));
        }
        Ok(())
    }

    pub fn is_off(&self) -> bool {
        self.brightness == 0
    }

    pub fn describe(&self) -> String {
        let colour = if self.effect.is_custom() {
            "per-key".to_string()
        } else {
            self.color.name().to_string()
        };
        format!(
            "{} · {} · brightness {}/50 · speed {} · direction {}",
            self.effect,
            colour,
            self.brightness,
            self.speed,
            self.direction.code()
        )
    }
}

/// Control Center's brightness slider is 0..=100 and sends slider / 2.
pub fn brightness_from_slider(percent: u8) -> u8 {
    percent.min(100) / 2
}

pub fn brightness_to_slider(raw: u8) -> u8 {
    raw.min(BRIGHTNESS_MAX) * 2
}

/// Control Center's speed slider is 0..=100 and sends 10 - slider / 10 (0 becomes 1).
pub fn speed_from_slider(percent: u8) -> u8 {
    let raw = 10 - percent.min(100) / 10;
    raw.max(SPEED_FASTEST)
}

/// Command 0x08 for validated settings.
pub fn encode_set(settings: &LightSettings) -> Result<Report, SettingsError> {
    settings.validate()?;
    Ok(finish([
        0,
        CMD_SET_EFFECT,
        0,
        settings.effect.code(),
        settings.speed,
        settings.brightness,
        settings.color.code(),
        settings.direction.code(),
        0,
    ]))
}

/// Command 0x88 request; the answer comes back through GetFeature.
pub fn encode_get() -> Report {
    finish([0, CMD_GET_EFFECT, 0, 0, 0, 0, 0, 0, 0])
}

pub fn encode_get_version() -> Report {
    finish([0, CMD_GET_VERSION, 0, 0, 0, 0, 0, 0, 0])
}

/// Command 0x12 for Custom slot 0..=4.
pub fn encode_set_picture(slot: u8) -> Result<Report, SettingsError> {
    if slot > 4 {
        return Err(SettingsError::CustomSlot(slot + 1));
    }
    Ok(finish([0, CMD_SET_PICTURE, 0, slot, 8, 0, 0, 0, 0]))
}

/// Command 0x92 for Custom slot 0..=4.
pub fn encode_get_picture(slot: u8) -> Result<Report, SettingsError> {
    if slot > 4 {
        return Err(SettingsError::CustomSlot(slot + 1));
    }
    Ok(finish([0, CMD_GET_PICTURE, 0, slot, 0, 0, 0, 0, 0]))
}

/// Parse a 0x88 answer (`[3]` effect, `[4]` speed, `[5]` brightness, `[6]` colour, `[7]` direction).
pub fn parse_effect(report: &Report) -> Option<LightSettings> {
    Some(LightSettings {
        effect: Effect::from_code(report[3])?,
        speed: report[4],
        brightness: report[5],
        color: Color::from_code(report[6])?,
        direction: Direction::from_code(report[7])?,
    })
}

/// Firmware version from a 0x80 answer: `"{buf[2]}.{buf[3]}"`.
pub fn parse_version(report: &Report) -> String {
    format!("{}.{}", report[2], report[3])
}

/// Fields the keyboard reports back. Custom slots ignore colour/speed/direction,
/// so only effect and brightness are compared there.
pub fn same_on_device(sent: &LightSettings, read: &LightSettings) -> bool {
    if sent.effect != read.effect || sent.brightness != read.brightness {
        return false;
    }
    if sent.effect.is_custom() {
        return true;
    }
    sent.color == read.color && sent.speed == read.speed && sent.direction == read.direction
}

/// A Custom-slot picture with every key set to one colour (`[0, r, g, b]` per entry).
pub fn solid_matrix(rgb: Rgb) -> [u8; MATRIX_LEN] {
    let mut m = [0u8; MATRIX_LEN];
    for entry in m.chunks_exact_mut(4) {
        entry.copy_from_slice(&[0, rgb.0, rgb.1, rgb.2]);
    }
    m
}

/// The eight 65-byte output reports (report id 0 + 64 bytes) for a picture.
pub fn picture_chunks(matrix: &[u8; MATRIX_LEN]) -> Vec<[u8; MATRIX_CHUNK + 1]> {
    matrix
        .chunks_exact(MATRIX_CHUNK)
        .map(|chunk| {
            let mut out = [0u8; MATRIX_CHUNK + 1];
            out[1..].copy_from_slice(chunk);
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(effect: Effect, color: Color, b: u8, sp: u8, d: Direction) -> LightSettings {
        LightSettings {
            effect,
            color,
            brightness: b,
            speed: sp,
            direction: d,
        }
    }

    #[test]
    fn static_red_matches_control_center_bytes() {
        // 8 + 1 + 5 + 50 + 1 + 1 = 66 -> 255 - 66 = 189
        let r = encode_set(&s(
            Effect::Static,
            Color::Red,
            50,
            5,
            Direction::LeftToRight,
        ))
        .unwrap();
        assert_eq!(r, [0x00, 0x08, 0x00, 0x01, 0x05, 0x32, 0x01, 0x01, 0xBD]);
    }

    #[test]
    fn controller_default_bytes() {
        // Static, speed 1, brightness 50, Red, L->R: 8+1+1+50+1+1 = 62 -> 193
        let r = encode_set(&LightSettings::default()).unwrap();
        assert_eq!(r, [0, 8, 0, 1, 1, 50, 1, 1, 193]);
    }

    #[test]
    fn wave_random_up_to_down() {
        // 8 + 3 + 10 + 25 + 8 + 4 = 58 -> 197
        let r = encode_set(&s(Effect::Wave, Color::Random, 25, 10, Direction::UpToDown)).unwrap();
        assert_eq!(r, [0, 8, 0, 3, 10, 25, 8, 4, 197]);
    }

    #[test]
    fn off_is_brightness_zero() {
        // 8 + 2 + 5 + 0 + 4 + 1 = 20 -> 235
        let r = encode_set(&s(
            Effect::Breathing,
            Color::Blue,
            0,
            5,
            Direction::LeftToRight,
        ))
        .unwrap();
        assert_eq!(r, [0, 8, 0, 2, 5, 0, 4, 1, 235]);
    }

    #[test]
    fn custom_slot_uses_51_to_55() {
        let r = encode_set(&s(
            Effect::Custom(5),
            Color::Red,
            50,
            5,
            Direction::LeftToRight,
        ))
        .unwrap();
        assert_eq!(r[3], 55);
        assert_eq!(r[8], 255u8.wrapping_sub(8 + 55 + 5 + 50 + 1 + 1));
    }

    #[test]
    fn checksum_wraps() {
        // 8 + 13 + 10 + 50 + 8 + 2 = 91 -> 164; force a wrap with large values too
        let r = encode_set(&s(
            Effect::Rotate,
            Color::Random,
            50,
            10,
            Direction::RightToLeft,
        ))
        .unwrap();
        assert_eq!(r[8], 164);
        let mut big = [0u8, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0];
        big[8] = checksum(&big);
        assert_eq!(big[8], 255u8.wrapping_sub(0xFE));
    }

    #[test]
    fn readback_and_version_requests() {
        assert_eq!(encode_get(), [0, 0x88, 0, 0, 0, 0, 0, 0, 0x77]);
        assert_eq!(encode_get_version(), [0, 0x80, 0, 0, 0, 0, 0, 0, 0x7F]);
    }

    #[test]
    fn picture_requests() {
        // 0x12 + 2 + 8 = 28 -> 227
        assert_eq!(
            encode_set_picture(2).unwrap(),
            [0, 0x12, 0, 2, 8, 0, 0, 0, 227]
        );
        // 0x92 + 4 = 150 -> 105
        assert_eq!(
            encode_get_picture(4).unwrap(),
            [0, 0x92, 0, 4, 0, 0, 0, 0, 105]
        );
        assert!(encode_set_picture(5).is_err());
    }

    #[test]
    fn solid_matrix_layout() {
        let m = solid_matrix(Rgb(1, 2, 3));
        assert_eq!(&m[..8], &[0, 1, 2, 3, 0, 1, 2, 3]);
        let chunks = picture_chunks(&m);
        assert_eq!(chunks.len(), 8);
        assert_eq!(chunks[0][0], 0);
        assert_eq!(&chunks[7][1..5], &[0, 1, 2, 3]);
    }

    #[test]
    fn parse_readback() {
        let r = [0, 0x08, 0, 6, 3, 40, 5, 1, 0];
        let p = parse_effect(&r).unwrap();
        assert_eq!(
            p,
            s(Effect::Ripple, Color::Orange, 40, 3, Direction::LeftToRight)
        );
        assert!(parse_effect(&[0, 0, 0, 99, 0, 0, 0, 1, 0]).is_none());
        assert_eq!(parse_version(&[0, 0, 1, 7, 0, 0, 0, 0, 0]), "1.7");
    }

    #[test]
    fn validation() {
        assert_eq!(
            encode_set(&s(
                Effect::Static,
                Color::Red,
                51,
                5,
                Direction::LeftToRight
            )),
            Err(SettingsError::Brightness(51))
        );
        assert_eq!(
            encode_set(&s(
                Effect::Static,
                Color::Red,
                50,
                0,
                Direction::LeftToRight
            )),
            Err(SettingsError::Speed(0))
        );
        assert_eq!(
            encode_set(&s(
                Effect::Static,
                Color::Red,
                50,
                11,
                Direction::LeftToRight
            )),
            Err(SettingsError::Speed(11))
        );
        assert!(encode_set(&s(Effect::Neon, Color::Red, 50, 5, Direction::LeftToRight)).is_err());
        assert!(encode_set(&s(Effect::Rotate, Color::Red, 50, 5, Direction::UpToDown)).is_err());
        assert!(encode_set(&s(
            Effect::Custom(6),
            Color::Red,
            50,
            5,
            Direction::LeftToRight
        ))
        .is_err());
    }

    #[test]
    fn effect_defaults_follow_control_center() {
        let d = LightSettings::effect_default(Effect::Breathing);
        assert_eq!(
            (d.brightness, d.speed, d.color, d.direction.code()),
            (50, 5, Color::Red, 1)
        );
        assert_eq!(
            LightSettings::effect_default(Effect::Wave).color,
            Color::Random
        );
        assert_eq!(
            LightSettings::effect_default(Effect::Raindrop).color,
            Color::Random
        );
    }

    #[test]
    fn slider_conversions() {
        assert_eq!(brightness_from_slider(100), 50);
        assert_eq!(brightness_from_slider(55), 27);
        assert_eq!(speed_from_slider(0), 10);
        assert_eq!(speed_from_slider(50), 5);
        assert_eq!(speed_from_slider(100), 1);
        assert_eq!(speed_from_slider(95), 1);
    }

    #[test]
    fn effect_table_round_trips() {
        assert_eq!(Effect::ALL.len(), 18);
        for e in Effect::ALL {
            assert_eq!(Effect::from_code(e.code()), Some(e));
            assert_eq!(Effect::from_key(e.key()), Some(e));
        }
        assert_eq!(Effect::from_key("STATIC"), Some(Effect::Static));
    }
}
