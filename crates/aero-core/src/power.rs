//! Battery charge policy and the NVIDIA power config switch.
//!
//! Each setter sends one byte and is checked with its getter. These writes do
//! not touch the fans, so a failure leaves the fan mode alone.

use aero_wmi::{AcpiWriter, Pacer, WriteParams};

use crate::apply::ApplyError;

const POLICY_STANDARD: u8 = 0;
const POLICY_CUSTOM: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargePolicy {
    Standard,
    /// Stop charging at this percent, 60..=100.
    Custom(u8),
}

pub fn set_charge_policy<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    policy: ChargePolicy,
) -> Result<(), ApplyError> {
    match policy {
        ChargePolicy::Standard => set_checked(
            writer,
            pacer,
            "SetChargePolicy",
            "GetChargePolicy",
            POLICY_STANDARD,
        ),
        ChargePolicy::Custom(stop) => {
            if !(60..=100).contains(&stop) {
                return Err(ApplyError::InvalidCharge(stop));
            }
            set_checked(
                writer,
                pacer,
                "SetChargePolicy",
                "GetChargePolicy",
                POLICY_CUSTOM,
            )?;
            pacer.pause();
            set_checked(writer, pacer, "SetChargeStop", "GetChargeStop", stop)
        }
    }
}

pub fn set_nv_power_config<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    on: bool,
) -> Result<(), ApplyError> {
    set_checked(
        writer,
        pacer,
        "SetNvPowerConfig",
        "GetNvPowerConfig",
        u8::from(on),
    )
}

fn set_checked<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    setter: &'static str,
    getter: &'static str,
    value: u8,
) -> Result<(), ApplyError> {
    writer.set(setter, &WriteParams::Data(value))?;
    pacer.pause();
    let actual = writer
        .get(getter, &[])?
        .get("Data")
        .and_then(|data| data.as_u32());
    if actual == Some(u32::from(value)) {
        Ok(())
    } else {
        Err(ApplyError::ReadBack {
            getter,
            expected: value,
            actual,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aero_wmi::{MockAcpi, NoPacer};

    fn sets(mock: &MockAcpi) -> Vec<(String, Vec<(String, u8)>)> {
        mock.calls
            .iter()
            .filter(|call| call.method.starts_with("Set"))
            .map(|call| (call.method.clone(), call.params.clone()))
            .collect()
    }

    #[test]
    fn custom_sends_policy_four_then_the_stop_percent() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        set_charge_policy(&mut mock, &mut NoPacer, ChargePolicy::Custom(80)).unwrap();
        assert_eq!(
            sets(&mock),
            [
                ("SetChargePolicy".to_string(), vec![("Data".to_string(), 4)]),
                ("SetChargeStop".to_string(), vec![("Data".to_string(), 80)]),
            ]
        );
        assert_eq!((mock.charge_policy, mock.charge_stop), (4, 80));
    }

    #[test]
    fn standard_sends_policy_zero_only() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        mock.charge_policy = 4;
        set_charge_policy(&mut mock, &mut NoPacer, ChargePolicy::Standard).unwrap();
        assert_eq!(
            sets(&mock),
            [("SetChargePolicy".to_string(), vec![("Data".to_string(), 0)])]
        );
    }

    #[test]
    fn stop_outside_60_to_100_never_reaches_the_writer() {
        for stop in [59, 101] {
            let mut mock = MockAcpi::aero_17_yb_fixture();
            let err = set_charge_policy(&mut mock, &mut NoPacer, ChargePolicy::Custom(stop));
            assert!(matches!(err, Err(ApplyError::InvalidCharge(_))));
            assert!(mock.calls.is_empty());
        }
    }

    #[test]
    fn nv_power_config_is_read_back() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        set_nv_power_config(&mut mock, &mut NoPacer, true).unwrap();
        assert_eq!(mock.nv_power_config, 1);
        assert_eq!(mock.calls_named("GetNvPowerConfig").count(), 1);
    }
}
