use serde::Serialize;

use crate::error::WmiError;

/// Identity fields the probe prints before it touches fan methods.
///
/// BIOS strings are left empty unless a backend actually read them. The mock
/// does not invent a BIOS version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkuInfo {
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub system_family: Option<String>,
    pub system_sku: Option<String>,
    pub bios_version: Option<String>,
    pub bios_manufacturer: Option<String>,
    pub baseboard_product: Option<String>,
    pub source: String,
}

pub trait SkuSource {
    fn read_sku(&mut self) -> Result<SkuInfo, WmiError>;
}
