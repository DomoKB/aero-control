//! Apply fan settings, then read them back.
//!
//! A failure or a panic runs the Normal script, which hands the fans back to
//! the firmware. Retries are a second full upload, never a temperature loop.

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

use aero_wmi::{decode_fan_index, AcpiReader, AcpiWriter, Pacer, WmiError, FAN_CURVE_SLOTS};
use thiserror::Error;
use tracing::debug;

use crate::curve::{planned_slots, CurveIssue, FanCurve};
use crate::mode::{fan_point_writes, script_for, FanMode, ModeScript, PlannedWrite};
use crate::profile::Profile;
use crate::scale::clamp_fixed_duty;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    ResumeAutomatic,
    ResumeSuspend,
    PowerStatusChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyReason {
    Startup,
    User,
    Resume,
    AcDcChanged,
    Drift,
}

pub fn reason_for(event: PowerEvent) -> ApplyReason {
    match event {
        PowerEvent::ResumeAutomatic | PowerEvent::ResumeSuspend => ApplyReason::Resume,
        PowerEvent::PowerStatusChange => ApplyReason::AcDcChanged,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotDiff {
    pub index: u8,
    pub expected_temp: u8,
    pub expected_duty: u8,
    pub actual_temp: Option<u8>,
    pub actual_duty: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyReport {
    pub attempts: u8,
    pub warnings: Vec<String>,
    pub restored_auto: bool,
    pub diffs: Vec<SlotDiff>,
}

#[derive(Debug, Error)]
pub enum ApplyError {
    #[error("the curve is not valid")]
    InvalidCurve(Vec<CurveIssue>),
    #[error("custom mode needs a curve")]
    CurveRequired,
    #[error("charge limit {0}% is outside 60..=100")]
    InvalidCharge(u8),
    #[error(transparent)]
    Wmi(#[from] WmiError),
    #[error("the firmware did not match after one retry")]
    VerifyFailed { report: ApplyReport },
    #[error("{reason} (firmware auto was restored)")]
    Restored { reason: String },
    #[error("firmware auto restore failed ({restore}) after: {reason}")]
    RestoreFailed { reason: String, restore: String },
}

struct Gate {
    started: bool,
}

impl Gate {
    fn before<P: Pacer>(&mut self, pacer: &mut P) {
        if self.started {
            pacer.pause();
        }
        self.started = true;
    }
}

fn guarded<W, P, T>(
    writer: &mut W,
    pacer: &mut P,
    body: impl FnOnce(&mut W, &mut P) -> Result<T, ApplyError>,
) -> Result<T, ApplyError>
where
    W: AcpiWriter,
    P: Pacer,
{
    let outcome = catch_unwind(AssertUnwindSafe(|| body(writer, pacer)));
    match outcome {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(finish_error(writer, pacer, err)),
        Err(payload) => {
            let _ = restore_firmware_auto(writer, pacer);
            resume_unwind(payload);
        }
    }
}

fn finish_error<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    err: ApplyError,
) -> ApplyError {
    match restore_firmware_auto(writer, pacer) {
        Ok(()) => ApplyError::Restored {
            reason: err.to_string(),
        },
        Err(restore) => ApplyError::RestoreFailed {
            reason: err.to_string(),
            restore: restore.to_string(),
        },
    }
}

pub fn restore_firmware_auto<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
) -> Result<(), ApplyError> {
    let mut gate = Gate { started: false };
    let mut warnings = Vec::new();
    run_script(
        writer,
        pacer,
        &mut gate,
        &crate::normal_script(),
        &mut warnings,
    )
}

pub fn upload_curve<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    curve: &FanCurve,
) -> Result<ApplyReport, ApplyError> {
    guarded(writer, pacer, |writer, pacer| {
        upload_inner(writer, pacer, curve)
    })
}

fn upload_inner<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    curve: &FanCurve,
) -> Result<ApplyReport, ApplyError> {
    let mut warnings = Vec::new();
    let mut gate = Gate { started: false };
    run_custom_once(writer, pacer, &mut gate, curve, &mut warnings)?;
    let diffs = read_curve(writer, pacer, &mut gate, curve)?;
    if diffs.is_empty() {
        return Ok(ApplyReport {
            attempts: 1,
            warnings,
            restored_auto: false,
            diffs,
        });
    }
    warnings.push("readback mismatched; retrying the full upload once".to_string());
    run_custom_once(writer, pacer, &mut gate, curve, &mut warnings)?;
    let diffs = read_curve(writer, pacer, &mut gate, curve)?;
    if diffs.is_empty() {
        return Ok(ApplyReport {
            attempts: 2,
            warnings,
            restored_auto: false,
            diffs,
        });
    }
    Err(ApplyError::VerifyFailed {
        report: ApplyReport {
            attempts: 2,
            warnings,
            restored_auto: false,
            diffs,
        },
    })
}

fn run_custom_once<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    gate: &mut Gate,
    curve: &FanCurve,
    warnings: &mut Vec<String>,
) -> Result<(), ApplyError> {
    let prefix = crate::custom_prefix();
    run_script(writer, pacer, gate, &prefix, warnings)?;
    for write in fan_point_writes(curve) {
        exec_write(writer, pacer, gate, &write, warnings)?;
    }
    Ok(())
}

fn read_curve<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    gate: &mut Gate,
    curve: &FanCurve,
) -> Result<Vec<SlotDiff>, ApplyError> {
    let expected = planned_slots(curve);
    let mut diffs = Vec::new();
    for index in 0..FAN_CURVE_SLOTS {
        gate.before(pacer);
        debug!(index, "GetFanIndexValue");
        let output = writer.get("GetFanIndexValue", &[("Index", index as u8)])?;
        let reading = decode_fan_index(index as u8, &output);
        let (expected_temp, expected_duty) = expected[index];
        let actual_temp = reading.temperature_c;
        let actual_duty = reading.duty_raw;
        if actual_temp != Some(expected_temp) || actual_duty != Some(expected_duty) {
            diffs.push(SlotDiff {
                index: index as u8,
                expected_temp,
                expected_duty,
                actual_temp,
                actual_duty,
            });
        }
    }
    Ok(diffs)
}

pub fn apply_mode<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    mode: FanMode,
    fixed_raw: u8,
    expert: bool,
) -> Result<ApplyReport, ApplyError> {
    if mode == FanMode::Custom {
        return Err(ApplyError::CurveRequired);
    }
    let duty = match mode {
        FanMode::Max => 229,
        FanMode::Fixed => clamp_fixed_duty(fixed_raw, expert),
        _ => fixed_raw,
    };
    guarded(writer, pacer, |writer, pacer| {
        apply_mode_inner(writer, pacer, mode, duty)
    })
}

fn apply_mode_inner<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    mode: FanMode,
    duty: u8,
) -> Result<ApplyReport, ApplyError> {
    let script = script_for(mode, duty);
    let mut warnings = Vec::new();
    if script.provisional {
        warnings.push(script.notes.to_string());
    }
    let mut gate = Gate { started: false };
    run_script(writer, pacer, &mut gate, &script, &mut warnings)?;
    let mut diffs = read_flags(writer, pacer, &mut gate, mode, duty)?;
    if diffs.is_empty() {
        return Ok(ApplyReport {
            attempts: 1,
            warnings,
            restored_auto: false,
            diffs,
        });
    }
    warnings.push("mode readback mismatched; retrying the full script once".to_string());
    run_script(writer, pacer, &mut gate, &script, &mut warnings)?;
    diffs = read_flags(writer, pacer, &mut gate, mode, duty)?;
    if diffs.is_empty() {
        return Ok(ApplyReport {
            attempts: 2,
            warnings,
            restored_auto: false,
            diffs,
        });
    }
    Err(ApplyError::VerifyFailed {
        report: ApplyReport {
            attempts: 2,
            warnings,
            restored_auto: false,
            diffs,
        },
    })
}

fn read_flags<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    gate: &mut Gate,
    mode: FanMode,
    duty: u8,
) -> Result<Vec<SlotDiff>, ApplyError> {
    let (auto, fixed, step, quiet, speed) = match mode {
        FanMode::Normal | FanMode::Gaming => (1, 0, 0, 0, None),
        FanMode::Quiet => (1, 0, 0, 1, None),
        FanMode::Max | FanMode::Fixed => (0, 1, 1, 0, Some(duty)),
        FanMode::Custom => (0, 0, 1, 0, None),
    };
    let mut diffs = Vec::new();
    check_flag(writer, pacer, gate, "GetAutoFanStatus", 0, auto, &mut diffs)?;
    check_flag(
        writer,
        pacer,
        gate,
        "GetFixedFanStatus",
        1,
        fixed,
        &mut diffs,
    )?;
    check_flag(writer, pacer, gate, "GetStepFanStatus", 2, step, &mut diffs)?;
    check_flag(writer, pacer, gate, "GetSuperQuiet", 3, quiet, &mut diffs)?;
    if let Some(duty) = speed {
        check_flag(writer, pacer, gate, "GetFixedFanSpeed", 4, duty, &mut diffs)?;
        check_flag(writer, pacer, gate, "GetGPUFanDuty", 5, duty, &mut diffs)?;
    }
    Ok(diffs)
}

fn check_flag<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    gate: &mut Gate,
    method: &str,
    index: u8,
    expected: u8,
    diffs: &mut Vec<SlotDiff>,
) -> Result<(), ApplyError> {
    gate.before(pacer);
    match writer.get(method, &[]) {
        Ok(output) => {
            let actual = output.first(&["Data"]).and_then(|value| value.as_u8());
            if actual != Some(expected) {
                diffs.push(SlotDiff {
                    index,
                    expected_temp: 0,
                    expected_duty: expected,
                    actual_temp: None,
                    actual_duty: actual,
                });
            }
            Ok(())
        }
        // GetSuperQuiet is not in the Aero 16 MOF. A missing getter is not a mismatch.
        Err(WmiError::MethodNotFound { .. }) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn run_script<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    gate: &mut Gate,
    script: &ModeScript,
    warnings: &mut Vec<String>,
) -> Result<(), ApplyError> {
    for write in &script.writes {
        exec_write(writer, pacer, gate, write, warnings)?;
    }
    Ok(())
}

fn exec_write<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    gate: &mut Gate,
    write: &PlannedWrite,
    warnings: &mut Vec<String>,
) -> Result<(), ApplyError> {
    gate.before(pacer);
    debug!(method = write.method, "WMI set");
    match writer.set(write.method, &write.params) {
        Ok(outcome) => {
            if let Some(message) = outcome.threw {
                warnings.push(format!(
                    "{}: WMI returned an error ({message}); the call is kept and checked with a getter",
                    write.method
                ));
            }
            Ok(())
        }
        Err(WmiError::MethodNotFound { .. }) if write.optional => {
            warnings.push(format!(
                "{} is not on this firmware and was skipped",
                write.method
            ));
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

pub fn apply_profile<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    profile: &Profile,
    reason: ApplyReason,
) -> Result<ApplyReport, ApplyError> {
    if let Some(limit) = profile.charge_limit {
        if !(60..=100).contains(&limit) {
            return Err(ApplyError::InvalidCharge(limit));
        }
    }
    debug!(profile = %profile.name, ?reason, "apply profile");
    let mut report = match profile.fan_mode {
        FanMode::Custom => {
            let curve = profile.fan_curve().map_err(ApplyError::InvalidCurve)?;
            upload_curve(writer, pacer, &curve)?
        }
        mode => {
            let raw = profile
                .fixed_duty_raw
                .unwrap_or_else(|| crate::percent_to_raw(profile.fixed_duty_percent.unwrap_or(50)));
            apply_mode(writer, pacer, mode, raw, profile.expert_mode)?
        }
    };
    if profile.cpu_pl1_w.is_some() || profile.cpu_pl2_w.is_some() {
        report
            .warnings
            .push("CPU PL1/PL2 is stored only; the XTU bridge is not implemented".to_string());
    }
    if profile.dynamic_boost.is_some()
        || profile.nv_power_config.is_some()
        || profile.nv_thermal_target.is_some()
        || profile.ai_boost.is_some()
        || profile.charge_limit.is_some()
    {
        report.warnings.push(
            "GPU power and battery charge fields are stored only; they are not sent to WMI yet"
                .to_string(),
        );
    }
    let _ = reason;
    Ok(report)
}

/// Getter-only comparison of the chip table with `expected`.
///
/// This does not change fan speeds. A caller that wants to repair drift calls
/// [`upload_curve`] afterwards.
pub fn curve_drift<R: AcpiReader>(
    reader: &mut R,
    expected: &FanCurve,
) -> Result<Vec<SlotDiff>, ApplyError> {
    let slots = planned_slots(expected);
    let mut diffs = Vec::new();
    for index in 0..FAN_CURVE_SLOTS {
        let output = reader.get("GetFanIndexValue", &[("Index", index as u8)])?;
        let reading = decode_fan_index(index as u8, &output);
        let (expected_temp, expected_duty) = slots[index];
        if reading.temperature_c != Some(expected_temp) || reading.duty_raw != Some(expected_duty) {
            diffs.push(SlotDiff {
                index: index as u8,
                expected_temp,
                expected_duty,
                actual_temp: reading.temperature_c,
                actual_duty: reading.duty_raw,
            });
        }
    }
    Ok(diffs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::CurvePoint;
    use crate::percent_to_raw;
    use crate::preset::curve_from_percent;
    use aero_wmi::{CountPacer, MockAcpi, NoPacer, WriteParams};
    use std::panic::{catch_unwind, AssertUnwindSafe};

    fn sample() -> FanCurve {
        curve_from_percent(&[(45, 35), (60, 45), (75, 60), (85, 80), (90, 100)])
    }

    fn methods(mock: &MockAcpi) -> Vec<String> {
        mock.calls.iter().map(|call| call.method.clone()).collect()
    }

    #[test]
    fn upload_sends_fifteen_points_then_reads_them_back() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        let mut pacer = CountPacer::default();
        let curve = sample();
        let report = upload_curve(&mut mock, &mut pacer, &curve).unwrap();
        assert_eq!(report.attempts, 1);
        assert!(!report.restored_auto);
        assert!(report.diffs.is_empty());

        let sets: Vec<_> = mock.calls_named("SetFanIndexValue").collect();
        assert_eq!(sets.len(), 15);
        let slots = planned_slots(&curve);
        for (index, call) in sets.iter().enumerate() {
            let params = &call.params;
            assert_eq!(params[0], ("Index".to_string(), index as u8));
            assert_eq!(params[1], ("Temperture".to_string(), slots[index].0));
            assert_eq!(params[2], ("Value".to_string(), slots[index].1));
        }
        assert!(slots[5..].iter().all(|slot| *slot == (0, 0)));
        assert_eq!(mock.calls_named("GetFanIndexValue").count(), 15);
        assert_eq!(pacer.pauses, mock.calls.len() - 1);
        assert_eq!(mock.auto, 0);
        assert!(methods(&mock).iter().all(|method| method != "TurnOffFan"));
    }

    #[test]
    fn custom_prefix_clears_step_auto_and_fixed_before_the_points() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        upload_curve(&mut mock, &mut NoPacer, &sample()).unwrap();
        let names = methods(&mock);
        let step = names
            .iter()
            .position(|name| name == "SetStepFanStatus")
            .unwrap();
        let first_point = names
            .iter()
            .position(|name| name == "SetFanIndexValue")
            .unwrap();
        assert!(step < first_point);
        assert!(names[..first_point].contains(&"SetCurrentFanStep".to_string()));
        assert!(names[..first_point].contains(&"SetAutoFanStatus".to_string()));
        assert!(names[..first_point].contains(&"SetFixedFanStatus".to_string()));
        assert!(names[..first_point].contains(&"SetFanCustomStatus".to_string()));
    }

    #[test]
    fn mismatch_retries_once_then_restores_auto() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        mock.corrupt_fan_index_reads = 1_000;
        let err = upload_curve(&mut mock, &mut NoPacer, &sample()).unwrap_err();
        assert!(matches!(err, ApplyError::Restored { .. }));
        assert_eq!(mock.calls_named("SetFanIndexValue").count(), 30);
        assert_eq!(mock.auto, 1);
        assert_eq!(mock.fixed, 0);
        assert_eq!(mock.step, 0);
    }

    #[test]
    fn one_bad_readback_is_repaired_by_the_second_full_upload() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        mock.corrupt_fan_index_reads = 15;
        let report = upload_curve(&mut mock, &mut NoPacer, &sample()).unwrap();
        assert_eq!(report.attempts, 2);
        assert!(!report.restored_auto);
        assert_eq!(mock.calls_named("SetFanIndexValue").count(), 30);
        assert_eq!(mock.curve[0], (45, percent_to_raw(35)));
    }

    #[test]
    fn an_invalid_curve_never_reaches_the_writer() {
        let mock = MockAcpi::aero_17_yb_fixture();
        let bad = FanCurve::try_new(vec![
            CurvePoint {
                temp_c: 0,
                duty_raw: 0,
            },
            CurvePoint {
                temp_c: 90,
                duty_raw: 229,
            },
        ]);
        assert!(bad.is_err());
        assert!(mock.calls.is_empty());
    }

    #[test]
    fn setter_throw_is_kept_and_verified() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        mock.throw_setters.insert("SetStepFanStatus".to_string());
        let report = upload_curve(&mut mock, &mut NoPacer, &sample()).unwrap();
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("SetStepFanStatus")));
        assert_eq!(mock.step, 1);
    }

    #[test]
    fn missing_custom_cap_method_is_skipped() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        mock.custom_cap = 1;
        mock.missing_setters
            .insert("SetFanCustomStatus".to_string());
        upload_curve(&mut mock, &mut NoPacer, &sample()).unwrap();
        assert_eq!(mock.custom_cap, 1);
        assert!(mock.calls_named("SetFanCustomStatus").count() >= 1);
    }

    #[test]
    fn panic_during_upload_restores_auto() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        mock.panic_on_set = Some("SetFanIndexValue".to_string());
        let curve = sample();
        let panicked = catch_unwind(AssertUnwindSafe(|| {
            let _ = upload_curve(&mut mock, &mut NoPacer, &curve);
        }));
        assert!(panicked.is_err());
        assert_eq!(mock.auto, 1);
        assert_eq!(mock.step, 0);
        assert_eq!(mock.fixed, 0);
    }

    #[test]
    fn fixed_duty_obeys_the_floor_until_expert_mode() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        apply_mode(&mut mock, &mut NoPacer, FanMode::Fixed, 10, false).unwrap();
        assert_eq!(mock.fixed_speed, 80);
        assert_eq!(mock.gpu_duty, 80);
        apply_mode(&mut mock, &mut NoPacer, FanMode::Fixed, 10, true).unwrap();
        assert_eq!(mock.fixed_speed, 10);
    }

    #[test]
    fn max_sets_both_fans_to_229() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        apply_mode(&mut mock, &mut NoPacer, FanMode::Max, 0, false).unwrap();
        assert_eq!(mock.fixed_speed, 229);
        assert_eq!(mock.gpu_duty, 229);
        assert_eq!(mock.auto, 0);
        assert_eq!(mock.fixed, 1);
    }

    #[test]
    fn quiet_sets_super_quiet_without_a_curve_upload() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        apply_mode(&mut mock, &mut NoPacer, FanMode::Quiet, 0, false).unwrap();
        assert_eq!(mock.super_quiet, 1);
        assert_eq!(mock.auto, 1);
        assert_eq!(mock.calls_named("SetFanIndexValue").count(), 0);
    }

    #[test]
    fn drift_check_is_getters_only() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        let curve = sample();
        upload_curve(&mut mock, &mut NoPacer, &curve).unwrap();
        mock.calls.clear();
        mock.curve[0].1 = mock.curve[0].1.wrapping_add(1);
        let diffs = curve_drift(&mut mock, &curve).unwrap();
        assert!(!diffs.is_empty());
        assert!(mock
            .calls
            .iter()
            .all(|call| call.method == "GetFanIndexValue"));
        assert!(mock
            .calls
            .iter()
            .all(|call| call.class == aero_wmi::GET_CLASS));
    }

    #[test]
    fn resume_and_ac_change_rerun_the_whole_script() {
        use crate::builtin_config;
        let config = builtin_config();
        let mut mock = MockAcpi::aero_17_yb_fixture();
        let on_ac = config.profile_for_power(true).unwrap();
        apply_profile(
            &mut mock,
            &mut NoPacer,
            on_ac,
            reason_for(PowerEvent::ResumeAutomatic),
        )
        .unwrap();
        assert_eq!(on_ac.fan_mode, FanMode::Gaming);
        let gaming_sets = mock
            .calls
            .iter()
            .filter(|call| call.class == aero_wmi::SET_CLASS)
            .count();
        mock.calls.clear();
        let on_battery = config.profile_for_power(false).unwrap();
        apply_profile(
            &mut mock,
            &mut NoPacer,
            on_battery,
            reason_for(PowerEvent::PowerStatusChange),
        )
        .unwrap();
        assert_eq!(on_battery.fan_mode, FanMode::Quiet);
        assert_eq!(mock.super_quiet, 1);
        let quiet_sets = mock
            .calls
            .iter()
            .filter(|call| call.class == aero_wmi::SET_CLASS)
            .count();
        assert!(gaming_sets > 0 && quiet_sets > 0);
    }

    #[test]
    fn stored_cpu_power_limits_do_not_create_wmi_writes() {
        let mut profile = crate::builtin_config()
            .profiles
            .into_iter()
            .find(|profile| profile.fan_mode == FanMode::Normal)
            .unwrap();
        profile.cpu_pl1_w = Some(45);
        profile.cpu_pl2_w = Some(90);
        profile.dynamic_boost = Some(true);
        let mut mock = MockAcpi::aero_17_yb_fixture();
        let report =
            apply_profile(&mut mock, &mut NoPacer, &profile, ApplyReason::Startup).unwrap();
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("XTU")));
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("not sent")));
        assert!(mock.calls.iter().all(|call| {
            call.method.starts_with("Set")
                && call.method != "SetNvPowerConfig"
                && call.method != "SetDynamicBoostStatus"
                || call.method.starts_with("Get")
        }));
    }

    #[test]
    fn charge_limit_outside_60_to_100_is_rejected_before_any_call() {
        let mut profile = crate::builtin_config().profile("Normal").unwrap().clone();
        profile.charge_limit = Some(59);
        let mut mock = MockAcpi::aero_17_yb_fixture();
        let err = apply_profile(&mut mock, &mut NoPacer, &profile, ApplyReason::User).unwrap_err();
        assert!(matches!(err, ApplyError::InvalidCharge(59)));
        assert!(mock.calls.is_empty());
    }

    #[test]
    fn reason_mapping() {
        assert_eq!(reason_for(PowerEvent::ResumeAutomatic), ApplyReason::Resume);
        assert_eq!(reason_for(PowerEvent::ResumeSuspend), ApplyReason::Resume);
        assert_eq!(
            reason_for(PowerEvent::PowerStatusChange),
            ApplyReason::AcDcChanged
        );
    }

    #[test]
    fn writer_records_data_under_the_mof_name() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        apply_mode(&mut mock, &mut NoPacer, FanMode::Normal, 0, false).unwrap();
        let call = mock.calls_named("SetAutoFanStatus").last().unwrap();
        assert_eq!(call.params, vec![("Data".to_string(), 1)]);
        let _ = WriteParams::Data(1);
    }
}
