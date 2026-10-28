use crate::catalog::{find_setter, NEVER_CALL};
use crate::error::WmiError;
use crate::value::WmiOutput;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassStatus {
    Present,
    Missing { detail: String },
}

/// Read-only view of `GB_WMIACPI_Get`.
///
/// Implementations must not invoke `GB_WMIACPI_Set`. [`is_setter_name`] is how
/// the mock and the Windows reader reject a setter that was passed by mistake.
pub trait AcpiReader {
    fn class_status(&mut self, class: &str) -> Result<ClassStatus, WmiError>;

    fn get(&mut self, method: &str, params: &[(&str, u8)]) -> Result<WmiOutput, WmiError>;
}

pub fn is_setter_name(method: &str) -> bool {
    method.starts_with("Set") || NEVER_CALL.contains(&method) || find_setter(method).is_some()
}

pub fn refuse_setter(method: &str) -> Result<(), WmiError> {
    if is_setter_name(method) {
        Err(WmiError::Refused {
            method: method.to_string(),
            reason: "readers never call GB_WMIACPI_Set",
        })
    } else {
        Ok(())
    }
}
