//! Lighting for the laptop's built-in keyboard, matching Gigabyte Control Center's
//! FusionKeyboard "Ite" path (USB 1044:7A3F on the Aero 17 YB).
//!
//! Written from protocol facts only: a 9-byte HID feature report
//! `[report id 0, cmd, 0, a, b, c, d, e, checksum]` where
//! `checksum = 255 - (sum of bytes 1..=7) mod 256`.

pub mod mock;
pub mod profile;
pub mod proto;
#[cfg(windows)]
pub mod windows_hid;

pub use proto::{Color, Direction, Effect, LightSettings, Rgb, SettingsError};

use std::fmt;

/// Vendor IDs Control Center accepts for this keyboard family.
pub const VENDOR_IDS: [u16; 2] = [0x04D9, 0x1044];
/// Product IDs Control Center drives with the 9-byte "Ite" protocol.
/// 0x7A3F is the US built-in keyboard on this Aero 17 YB.
pub const ITE_PRODUCT_IDS: [u16; 5] = [0x7A38, 0x7A39, 0x7A3A, 0x7A3B, 0x7A3F];
/// The feature report length Control Center requires before it uses a handle.
pub const FEATURE_LEN: usize = 9;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    pub path: String,
    pub firmware: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KbdError {
    NotFound,
    Invalid(SettingsError),
    Io(String),
    /// The write returned success but the readback disagrees.
    Mismatch {
        sent: LightSettings,
        read: LightSettings,
    },
}

impl fmt::Display for KbdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KbdError::NotFound => write!(
                f,
                "built-in keyboard (1044:7A3F, 9-byte feature collection) not found"
            ),
            KbdError::Invalid(e) => write!(f, "invalid settings: {e}"),
            KbdError::Io(e) => write!(f, "keyboard I/O failed: {e}"),
            KbdError::Mismatch { sent, read } => write!(
                f,
                "keyboard accepted the write but reads back {} (sent {})",
                read.describe(),
                sent.describe()
            ),
        }
    }
}

impl std::error::Error for KbdError {}

impl From<SettingsError> for KbdError {
    fn from(e: SettingsError) -> Self {
        KbdError::Invalid(e)
    }
}

/// What a successful apply proved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verified {
    /// Readback matched every field sent.
    ReadBack(LightSettings),
    /// The write succeeded but this backend cannot read back.
    WriteOnly,
}

pub trait KeyboardBackend: Send {
    fn label(&self) -> &'static str;
    fn info(&self) -> &DeviceInfo;
    /// Send command 0x08 and verify with command 0x88.
    fn apply(&mut self, settings: &LightSettings) -> Result<Verified, KbdError>;
    /// Command 0x88: the effect currently running.
    fn read(&mut self) -> Result<LightSettings, KbdError>;
    /// Command 0x92 + eight 64-byte input reads: a Custom slot's per-key matrix.
    fn read_custom(&mut self, slot: u8) -> Result<[u8; proto::MATRIX_LEN], KbdError>;
    /// Command 0x12 + eight 64-byte output writes, then verify with `read_custom`.
    fn write_custom(&mut self, slot: u8, matrix: &[u8; proto::MATRIX_LEN]) -> Result<(), KbdError>;
}

/// Shared apply logic: send, then compare readback field by field.
pub fn verify(sent: &LightSettings, read: LightSettings) -> Result<Verified, KbdError> {
    if proto::same_on_device(sent, &read) {
        Ok(Verified::ReadBack(read))
    } else {
        Err(KbdError::Mismatch { sent: *sent, read })
    }
}

/// Open the real keyboard on Windows; elsewhere always `NotFound`.
pub fn open_real() -> Result<Box<dyn KeyboardBackend>, KbdError> {
    #[cfg(windows)]
    {
        windows_hid::HidKeyboard::open().map(|k| Box::new(k) as Box<dyn KeyboardBackend>)
    }
    #[cfg(not(windows))]
    {
        Err(KbdError::NotFound)
    }
}
