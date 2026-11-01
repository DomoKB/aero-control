use crate::catalog::NEVER_CALL;
use crate::error::WmiError;
use crate::read::AcpiReader;

/// Successful setter output. WMI transport failure does not establish whether firmware applied the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetOutcome {
    pub threw: Option<String>,
    pub returned: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteParams {
    /// MOF in-parameter name is `Data` (the description text is not the name).
    Data(u8),
    /// `SetFanIndexValue`. The temperature parameter is spelled `Temperture`.
    FanPoint { index: u8, temp: u8, duty: u8 },
}

impl WriteParams {
    pub fn pairs(&self) -> Vec<(&'static str, u8)> {
        match *self {
            Self::Data(value) => vec![("Data", value)],
            Self::FanPoint { index, temp, duty } => {
                vec![("Index", index), ("Temperture", temp), ("Value", duty)]
            }
        }
    }
}

pub trait AcpiWriter: AcpiReader {
    fn set(&mut self, method: &str, params: &WriteParams) -> Result<SetOutcome, WmiError>;
}

pub fn refuse_forbidden(method: &str) -> Result<(), WmiError> {
    if NEVER_CALL.contains(&method) {
        Err(WmiError::Refused {
            method: method.to_string(),
            reason: "aero-control does not call this method",
        })
    } else {
        Ok(())
    }
}
