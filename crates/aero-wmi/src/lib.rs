//! Gigabyte `GB_WMIACPI` transport, called through the WMI data-block API.
//!
//! Fan speeds are decided by the laptop's embedded controller. This crate only
//! sends the same get/set commands Gigabyte Control Center sends. It never
//! reads a temperature and writes a duty in a loop.
//!
//! The probe binary depends on this crate **without** the `writer` feature.
//! Setter methods are compiled out of that build, and [`AcpiReader::get`]
//! refuses any setter name even if one is passed in.

#![deny(unsafe_code)]

mod catalog;
mod decode;
mod diag;
mod error;
mod mock;
mod pace;
mod read;
mod sku;
mod value;

#[cfg(feature = "writer")]
mod write;

#[cfg(windows)]
mod platform;

pub use catalog::{
    decode_output, find_getter, find_setter, pack_input, Block, MethodSpec, Width, GETTERS,
    NEVER_CALL, SETTERS,
};
pub use decode::{decode_fan_index, decode_rpm, swap_u16, FanIndexReading, RpmOrder, RpmReading};
pub use diag::{AcpiDiag, MethodSignature, ParamField};
pub use error::WmiError;
pub use mock::MockAcpi;
pub use pace::{CountPacer, FixedPacer, NoPacer, Pacer, WMI_PACE};
pub use read::{is_setter_name, AcpiReader, ClassStatus};
pub use sku::{SkuInfo, SkuSource};
pub use value::{WmiNumber, WmiOutput};

#[cfg(feature = "writer")]
pub use write::{AcpiWriter, SetOutcome, WriteParams};

#[cfg(windows)]
pub use platform::{read_system_sku, WindowsReader};

#[cfg(all(windows, feature = "writer"))]
pub use platform::WindowsWriter;

pub const GET_CLASS: &str = "GB_WMIACPI_Get";
pub const SET_CLASS: &str = "GB_WMIACPI_Set";

/// How many slots the firmware fan table has. Indices are `0..15`.
pub const FAN_CURVE_SLOTS: usize = 15;
