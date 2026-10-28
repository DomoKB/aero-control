use serde::Serialize;

use crate::error::WmiError;

/// One parameter from a WMI method's in or out signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParamField {
    pub name: String,
    pub cim_type: i32,
    pub cim_type_name: String,
}

/// Instance path plus the in/out signature. This does not call the method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MethodSignature {
    pub class: String,
    pub method: String,
    pub instance_path: String,
    pub in_params: Vec<ParamField>,
    pub out_params: Vec<ParamField>,
}

pub trait AcpiDiag {
    fn method_signature(&mut self, class: &str, method: &str) -> Result<MethodSignature, WmiError>;
}
