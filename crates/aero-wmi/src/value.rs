use std::collections::BTreeMap;

use serde::Serialize;

/// A numeric WMI out-parameter, normalised from UI1/UI2/UI4/I2/I4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum WmiNumber {
    U8(u8),
    U16(u16),
    U32(u32),
    I32(i32),
}

impl WmiNumber {
    pub fn as_u32(self) -> Option<u32> {
        match self {
            Self::U8(v) => Some(u32::from(v)),
            Self::U16(v) => Some(u32::from(v)),
            Self::U32(v) => Some(v),
            Self::I32(v) if v >= 0 => Some(v as u32),
            Self::I32(_) => None,
        }
    }

    pub fn as_u8(self) -> Option<u8> {
        self.as_u32().and_then(|v| u8::try_from(v).ok())
    }
}

/// Named out-parameters from one method call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct WmiOutput {
    pub properties: BTreeMap<String, WmiNumber>,
}

impl WmiOutput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: &str, value: WmiNumber) {
        self.properties.insert(name.to_string(), value);
    }

    pub fn get(&self, name: &str) -> Option<WmiNumber> {
        self.properties.get(name).copied()
    }

    /// First present property, so callers can accept MOF typos and aliases.
    pub fn first(&self, names: &[&str]) -> Option<WmiNumber> {
        names.iter().find_map(|name| self.get(name))
    }
}
