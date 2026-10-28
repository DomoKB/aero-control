use thiserror::Error;

#[derive(Debug, Error)]
pub enum WmiError {
    #[error("WMI class {class} was not found: {detail}")]
    ClassMissing { class: String, detail: String },

    #[error("WMI method {method} was not found")]
    MethodNotFound { method: String },

    #[error("refusing to call {method}: {reason}")]
    Refused {
        method: String,
        reason: &'static str,
    },

    #[error("WMI call {method} failed: {detail}")]
    CallFailed { method: String, detail: String },

    #[error("WMI method {method} is missing in-parameter {name}")]
    MissingParam { method: String, name: &'static str },

    #[error("fan curve index {index} is outside 0..15")]
    IndexOutOfRange { index: u8 },

    #[error("this build has no Windows WMI backend ({0})")]
    NotAvailable(&'static str),
}
