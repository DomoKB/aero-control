//! Battery charge policy and NVIDIA power/thermal configuration.
//!
//! Charge/thermal settings have durable getters. NVIDIA power configuration
//! is a transient request; its getter cannot confirm the resulting GPU budget.
//! These writes do not touch the fans.

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
    // SmartDashboard gpustage0/gpustage1 on YB: select the NVIDIA
    // configuration, then disable the separate GPU boost flag (FESC).
    // TEMQ can be cleared within milliseconds after notification. A zero
    // getter value does not prove that Level 2 failed (or succeeded).
    let outcome = writer.set("SetNvPowerConfig", &WriteParams::Data(u8::from(on)))?;
    if let Some(detail) = outcome.threw {
        return Err(aero_wmi::WmiError::CallFailed {
            method: "SetNvPowerConfig".into(),
            detail,
        }
        .into());
    }
    pacer.pause();
    set_checked(writer, pacer, "SetAIBoostStatus", "GetAIBoostStatus", 0)?;
    pacer.pause();
    let actual = writer
        .get("GetNvPowerConfig", &[])?
        .get("Data")
        .and_then(|value| value.as_u32());
    if matches!(actual, Some(0 | 1)) {
        Ok(())
    } else {
        Err(ApplyError::Wmi(aero_wmi::WmiError::CallFailed {
            method: "GetNvPowerConfig".into(),
            detail: format!("unexpected request flag {actual:?}"),
        }))
    }
}

pub fn set_nv_thermal_target<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    quiet: bool,
) -> Result<(), ApplyError> {
    set_checked(
        writer,
        pacer,
        "SetNvThermalTarget",
        "GetNvThermalTarget",
        u8::from(quiet),
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
    check_value(writer, getter, value)
}

fn check_value<W: AcpiWriter>(
    writer: &mut W,
    getter: &'static str,
    value: u8,
) -> Result<(), ApplyError> {
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
        mock.ai_boost_status = 1;
        set_nv_power_config(&mut mock, &mut NoPacer, true).unwrap();
        assert_eq!(mock.nv_power_config, 1);
        assert_eq!(mock.ai_boost_status, 0);
        assert_eq!(mock.calls_named("GetNvPowerConfig").count(), 1);
        assert_eq!(
            mock.calls
                .iter()
                .map(|call| call.method.as_str())
                .collect::<Vec<_>>(),
            [
                "SetNvPowerConfig",
                "SetAIBoostStatus",
                "GetAIBoostStatus",
                "GetNvPowerConfig"
            ]
        );
    }

    #[test]
    fn a_cleared_gpu_request_is_not_a_durable_readback_failure() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        let fans = (
            mock.auto,
            mock.fixed,
            mock.step,
            mock.fixed_speed,
            mock.gpu_duty,
        );
        mock.consume_nv_power_requests = true;
        set_nv_power_config(&mut mock, &mut NoPacer, true).unwrap();
        assert_eq!(mock.nv_power_config, 0);
        assert_eq!(
            (
                mock.auto,
                mock.fixed,
                mock.step,
                mock.fixed_speed,
                mock.gpu_duty
            ),
            fans
        );
        assert_eq!(mock.calls_named("SetNvPowerConfig").count(), 1);
    }

    #[test]
    fn gpu_level_rejects_a_boost_clear_that_does_not_take_effect() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        mock.ai_boost_status = 1;
        mock.ignored_setters.insert("SetAIBoostStatus".into());
        assert!(matches!(
            set_nv_power_config(&mut mock, &mut NoPacer, true),
            Err(ApplyError::ReadBack {
                getter: "GetAIBoostStatus",
                ..
            })
        ));
        assert_eq!(mock.calls_named("GetNvPowerConfig").count(), 0);
    }

    #[test]
    fn gpu_request_transport_failure_stops_before_the_boost_clear() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        mock.throw_setters.insert("SetNvPowerConfig".into());
        assert!(set_nv_power_config(&mut mock, &mut NoPacer, true).is_err());
        assert_eq!(mock.calls_named("SetNvPowerConfig").count(), 1);
        assert_eq!(mock.calls_named("SetAIBoostStatus").count(), 0);
        assert_eq!(mock.calls_named("GetNvPowerConfig").count(), 0);

        struct LegacyTransport;
        impl aero_wmi::AcpiReader for LegacyTransport {
            fn class_status(
                &mut self,
                _: &str,
            ) -> Result<aero_wmi::ClassStatus, aero_wmi::WmiError> {
                Ok(aero_wmi::ClassStatus::Present)
            }
            fn get(
                &mut self,
                _: &str,
                _: &[(&str, u8)],
            ) -> Result<aero_wmi::WmiOutput, aero_wmi::WmiError> {
                panic!("an exception must stop before readback")
            }
        }
        impl AcpiWriter for LegacyTransport {
            fn set(
                &mut self,
                method: &str,
                _: &WriteParams,
            ) -> Result<aero_wmi::SetOutcome, aero_wmi::WmiError> {
                assert_eq!(method, "SetNvPowerConfig");
                Ok(aero_wmi::SetOutcome {
                    threw: Some("legacy transport error".into()),
                    returned: vec![],
                })
            }
        }
        assert!(
            set_nv_power_config(&mut LegacyTransport, &mut NoPacer, true)
                .unwrap_err()
                .to_string()
                .contains("legacy transport error")
        );
    }

    #[test]
    fn thermal_target_readback_rejects_a_silent_noop_without_retrying() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        mock.ignored_setters.insert("SetNvThermalTarget".into());
        assert!(matches!(
            set_nv_thermal_target(&mut mock, &mut NoPacer, true),
            Err(ApplyError::ReadBack {
                getter: "GetNvThermalTarget",
                ..
            })
        ));
        assert_eq!(mock.calls_named("SetNvThermalTarget").count(), 1);
        assert_eq!(mock.calls_named("GetNvThermalTarget").count(), 1);
        assert!(mock.calls.iter().all(|call| ![
            "SetFixedFanStatus",
            "SetAutoFanStatus",
            "SetStepFanStatus"
        ]
        .contains(&call.method.as_str())));
    }

    #[test]
    fn thermal_transport_failure_does_not_continue_to_readback_or_retry() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        mock.throw_setters.insert("SetNvThermalTarget".into());
        assert!(set_nv_thermal_target(&mut mock, &mut NoPacer, true).is_err());
        assert_eq!(mock.calls.len(), 1);
        assert_eq!(mock.calls[0].method, "SetNvThermalTarget");
    }
}
