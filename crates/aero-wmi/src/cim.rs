//! CIM type names and the VARIANT width used when putting a byte.
//!
//! The Windows call site turns [`CoercedInt`] into a real `VARIANT` and passes
//! the CIM type to `IWbemClassObject::Put`. Parameterless methods do not go
//! through this path: they are invoked with a NULL in-params pointer.

/// `CIM_UINT8`.
pub const CIM_UINT8: i32 = 17;
/// `CIM_SINT8`.
pub const CIM_SINT8: i32 = 16;
/// `CIM_UINT16`.
pub const CIM_UINT16: i32 = 18;
/// `CIM_SINT16`.
pub const CIM_SINT16: i32 = 2;
/// `CIM_UINT32`.
pub const CIM_UINT32: i32 = 19;
/// `CIM_SINT32`.
pub const CIM_SINT32: i32 = 3;
/// `CIM_STRING`.
pub const CIM_STRING: i32 = 8;

/// Strip `CIM_FLAG_ARRAY` (0x2000) so the base type can be matched.
pub fn cim_base(cim_type: i32) -> i32 {
    cim_type & 0x0fff
}

pub fn cim_type_name(cim_type: i32) -> &'static str {
    match cim_base(cim_type) {
        CIM_SINT16 => "sint16",
        CIM_SINT32 => "sint32",
        CIM_SINT8 => "sint8",
        CIM_UINT8 => "uint8",
        CIM_UINT16 => "uint16",
        CIM_UINT32 => "uint32",
        CIM_STRING => "string",
        0 => "empty",
        _ => "other",
    }
}

/// How a `u8` argument is stored in the in-params `VARIANT`.
///
/// `uint8` stays `VT_UI1`. Wider integers use the widths WMI actually accepts:
/// `uint16` is `VT_I4`, not `VT_UI2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoercedInt {
    Ui1(u8),
    I2(i16),
    I4(i32),
}

pub fn coerce_byte(value: u8, cim_type: i32) -> CoercedInt {
    match cim_base(cim_type) {
        CIM_UINT8 => CoercedInt::Ui1(value),
        CIM_SINT8 | CIM_SINT16 => CoercedInt::I2(i16::from(value)),
        _ => CoercedInt::I4(i32::from(value)),
    }
}

/// A method whose in-signature has no non-system properties must be executed
/// with a NULL in-params pointer. An empty spawned instance is what produced
/// `WBEM_E_INVALID_METHOD_PARAMETERS` (0x8004102F) on the AERO 17 YB.
pub fn pass_null_inparams(in_param_count: usize) -> bool {
    in_param_count == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uint8_stays_vt_ui1_and_uint16_becomes_vt_i4() {
        assert_eq!(coerce_byte(45, CIM_UINT8), CoercedInt::Ui1(45));
        assert_eq!(coerce_byte(45, CIM_UINT16), CoercedInt::I4(45));
        assert_eq!(coerce_byte(45, CIM_SINT32), CoercedInt::I4(45));
        assert_eq!(coerce_byte(45, CIM_SINT16), CoercedInt::I2(45));
    }

    #[test]
    fn parameterless_methods_pass_null() {
        assert!(pass_null_inparams(0));
        assert!(!pass_null_inparams(1));
    }

    #[test]
    fn type_names_cover_the_fan_mof() {
        assert_eq!(cim_type_name(CIM_UINT8), "uint8");
        assert_eq!(cim_type_name(CIM_UINT16), "uint16");
        assert_eq!(cim_type_name(CIM_UINT8 | 0x2000), "uint8");
    }
}
