//! In-memory keyboard for `--mock` and tests. It stores what was sent and
//! reads it back, like the firmware does for command 0x88.

use crate::proto::{self, LightSettings, MATRIX_LEN};
use crate::{verify, DeviceInfo, KbdError, KeyboardBackend, Verified};

pub struct MockKeyboard {
    info: DeviceInfo,
    current: LightSettings,
    pictures: [[u8; MATRIX_LEN]; 5],
    /// Every 9-byte feature report "sent", for tests.
    pub sent: Vec<proto::Report>,
    /// When set, the next apply reads back this instead (simulates a mismatch).
    pub corrupt_next: Option<LightSettings>,
}

impl Default for MockKeyboard {
    fn default() -> Self {
        Self::new()
    }
}

impl MockKeyboard {
    pub fn new() -> Self {
        MockKeyboard {
            info: DeviceInfo {
                vendor_id: 0x1044,
                product_id: 0x7A3F,
                path: "mock://keyboard".into(),
                firmware: Some("mock".into()),
            },
            current: LightSettings::default(),
            pictures: [[0u8; MATRIX_LEN]; 5],
            sent: Vec::new(),
            corrupt_next: None,
        }
    }
}

impl KeyboardBackend for MockKeyboard {
    fn label(&self) -> &'static str {
        "MOCK"
    }

    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn apply(&mut self, settings: &LightSettings) -> Result<Verified, KbdError> {
        let report = proto::encode_set(settings)?;
        self.sent.push(report);
        self.current = proto::parse_effect(&report).ok_or_else(|| KbdError::Io("encode".into()))?;
        let read = self.corrupt_next.take().unwrap_or(self.current);
        verify(settings, read)
    }

    fn read(&mut self) -> Result<LightSettings, KbdError> {
        self.sent.push(proto::encode_get());
        Ok(self.current)
    }

    fn read_custom(&mut self, slot: u8) -> Result<[u8; MATRIX_LEN], KbdError> {
        self.sent.push(proto::encode_get_picture(slot)?);
        Ok(self.pictures[slot as usize])
    }

    fn write_custom(&mut self, slot: u8, matrix: &[u8; MATRIX_LEN]) -> Result<(), KbdError> {
        self.sent.push(proto::encode_set_picture(slot)?);
        self.pictures[slot as usize] = *matrix;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{Color, Direction, Effect, Rgb};

    #[test]
    fn apply_sends_exact_report_and_verifies() {
        let mut kb = MockKeyboard::new();
        let s = LightSettings {
            effect: Effect::Marquee,
            color: Color::Purple,
            brightness: 30,
            speed: 2,
            direction: Direction::LeftToRight,
        };
        assert_eq!(kb.apply(&s), Ok(Verified::ReadBack(s)));
        // 8 + 5 + 2 + 30 + 6 + 1 = 52 -> 203
        assert_eq!(kb.sent, vec![[0, 8, 0, 5, 2, 30, 6, 1, 203]]);
        assert_eq!(kb.read().unwrap(), s);
    }

    #[test]
    fn invalid_settings_send_nothing() {
        let mut kb = MockKeyboard::new();
        let s = LightSettings {
            brightness: 60,
            ..LightSettings::default()
        };
        assert!(matches!(kb.apply(&s), Err(KbdError::Invalid(_))));
        assert!(kb.sent.is_empty());
    }

    #[test]
    fn mismatch_is_reported_not_hidden() {
        let mut kb = MockKeyboard::new();
        kb.corrupt_next = Some(LightSettings {
            brightness: 10,
            ..LightSettings::default()
        });
        assert!(matches!(
            kb.apply(&LightSettings::default()),
            Err(KbdError::Mismatch { .. })
        ));
    }

    #[test]
    fn custom_picture_round_trip() {
        let mut kb = MockKeyboard::new();
        let m = proto::solid_matrix(Rgb(9, 8, 7));
        kb.write_custom(3, &m).unwrap();
        assert_eq!(kb.read_custom(3).unwrap(), m);
    }
}
