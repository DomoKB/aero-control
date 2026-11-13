//! Apply fan settings, then read them back.
//!
//! A failure or a panic runs the Normal script, which hands the fans back to
//! the firmware. Retries are a second full upload, never a temperature loop.

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

use aero_wmi::{decode_fan_index, AcpiReader, AcpiWriter, Pacer, WmiError, FAN_CURVE_SLOTS};
use thiserror::Error;
use tracing::debug;

use crate::curve::{planned_slots, CurveIssue, FanCurve};
use crate::mode::{
    fan_point_writes, fixed_duty_script, script_for, FanMode, ModeScript, PlannedWrite,
};
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

/// Whether the NVIDIA power-request interface is available for this session.
/// The EC thermal-profile and fan commands remain usable without the GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuPower {
    Available,
    Unavailable,
}

const GPU_POWER_SKIPPED: &str = "NVIDIA GPU unavailable; GPU power request skipped.";

fn note_gpu_power_skipped(warnings: &mut Vec<String>) {
    if !warnings.iter().any(|warning| warning == GPU_POWER_SKIPPED) {
        warnings.push(GPU_POWER_SKIPPED.to_string());
    }
}

pub fn reason_for(event: PowerEvent) -> ApplyReason {
    match event {
        PowerEvent::ResumeAutomatic | PowerEvent::ResumeSuspend => ApplyReason::Resume,
        PowerEvent::PowerStatusChange => ApplyReason::AcDcChanged,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotDiff {
    /// Getter which produced the mismatch. Flags have no curve-slot meaning.
    pub getter: &'static str,
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

impl std::fmt::Display for ApplyReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (position, diff) in self.diffs.iter().enumerate() {
            if position > 0 {
                write!(f, "; ")?;
            }
            if diff.getter == "GetFanIndexValue" {
                write!(f, "{} slot {} returned temperature {:?}, duty {:?}; expected temperature {}, duty {}", diff.getter, diff.index, diff.actual_temp, diff.actual_duty, diff.expected_temp, diff.expected_duty)?;
            } else {
                write!(
                    f,
                    "{} returned {:?}; expected {}",
                    diff.getter, diff.actual_duty, diff.expected_duty
                )?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ApplyError {
    #[error("this profile requires the CPU power bridge; use the desktop app")]
    CpuBridgeRequired,
    #[error("the curve is not valid")]
    InvalidCurve(Vec<CurveIssue>),
    #[error("custom mode needs a curve")]
    CurveRequired,
    #[error("charge limit {0}% is outside 60..=100")]
    InvalidCharge(u8),
    #[error("charge policy must be Standard (0) or Custom (4)")]
    InvalidChargePolicy,
    #[error("{field} must be 0 or 1, got {value}")]
    InvalidSwitch { field: &'static str, value: u8 },
    #[error(transparent)]
    Wmi(#[from] WmiError),
    #[error("{getter} read back {actual:?}, expected {expected}")]
    ReadBack {
        getter: &'static str,
        expected: u8,
        actual: Option<u32>,
    },
    #[error("the firmware did not match after one retry: {report}")]
    VerifyFailed { report: ApplyReport },
    #[error("{reason} (firmware auto was restored)")]
    Restored { reason: String },
    #[error("firmware auto restore failed ({restore}) after: {reason}")]
    RestoreFailed { reason: String, restore: String },
}

struct Gate {
    started: bool,
    gpu: GpuPower,
}

impl Gate {
    fn new(gpu: GpuPower) -> Self {
        Self {
            started: false,
            gpu,
        }
    }

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
    gpu: GpuPower,
    body: impl FnOnce(&mut W, &mut P) -> Result<T, ApplyError>,
) -> Result<T, ApplyError>
where
    W: AcpiWriter,
    P: Pacer,
{
    let outcome = catch_unwind(AssertUnwindSafe(|| body(writer, pacer)));
    match outcome {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(finish_error(writer, pacer, gpu, err)),
        Err(payload) => {
            let _ = restore_firmware_auto_with_gpu(writer, pacer, gpu);
            resume_unwind(payload);
        }
    }
}

fn finish_error<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    gpu: GpuPower,
    err: ApplyError,
) -> ApplyError {
    match restore_firmware_auto_with_gpu(writer, pacer, gpu) {
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
    restore_firmware_auto_with_gpu(writer, pacer, GpuPower::Available)
}

pub fn restore_firmware_auto_with_gpu<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    gpu: GpuPower,
) -> Result<(), ApplyError> {
    let mut gate = Gate::new(gpu);
    let mut warnings = Vec::new();
    run_script(
        writer,
        pacer,
        &mut gate,
        &crate::normal_script(),
        &mut warnings,
    )?;
    let diffs = read_flags(writer, pacer, &mut gate, FanMode::Normal, 0)?;
    if diffs.is_empty() {
        Ok(())
    } else {
        Err(ApplyError::VerifyFailed {
            report: ApplyReport {
                attempts: 1,
                warnings,
                restored_auto: false,
                diffs,
            },
        })
    }
}

pub fn upload_curve<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    curve: &FanCurve,
) -> Result<ApplyReport, ApplyError> {
    upload_curve_with_gpu(writer, pacer, curve, GpuPower::Available)
}

pub fn upload_curve_with_gpu<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    curve: &FanCurve,
    gpu: GpuPower,
) -> Result<ApplyReport, ApplyError> {
    guarded(writer, pacer, gpu, |writer, pacer| {
        upload_inner(writer, pacer, &mut Gate::new(gpu), Vec::new(), curve)
    })
}

pub fn update_curve<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    curve: &FanCurve,
) -> Result<ApplyReport, ApplyError> {
    update_curve_with_gpu(writer, pacer, curve, GpuPower::Available)
}

pub fn update_curve_with_gpu<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    curve: &FanCurve,
    gpu: GpuPower,
) -> Result<ApplyReport, ApplyError> {
    guarded(writer, pacer, gpu, |writer, pacer| {
        let mut gate = Gate::new(gpu);
        let mut warnings = Vec::new();
        for write in fan_point_writes(curve) {
            exec_write(writer, pacer, &mut gate, &write, &mut warnings)?;
        }
        let mut diffs = read_curve(writer, pacer, &mut gate, curve)?;
        diffs.extend(read_flags(writer, pacer, &mut gate, FanMode::Custom, 0)?);
        if diffs.is_empty() {
            return Ok(ApplyReport {
                attempts: 1,
                warnings,
                restored_auto: false,
                diffs,
            });
        }
        warnings.push("curve readback mismatched; running the full upload".to_string());
        upload_inner(writer, pacer, &mut gate, warnings, curve)
    })
}

fn upload_inner<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    gate: &mut Gate,
    mut warnings: Vec<String>,
    curve: &FanCurve,
) -> Result<ApplyReport, ApplyError> {
    run_custom_once(writer, pacer, gate, curve, &mut warnings)?;
    let diffs = read_curve(writer, pacer, gate, curve)?;
    if diffs.is_empty() {
        return Ok(ApplyReport {
            attempts: 1,
            warnings,
            restored_auto: false,
            diffs,
        });
    }
    warnings.push("readback mismatched; retrying the full upload once".to_string());
    run_custom_once(writer, pacer, gate, curve, &mut warnings)?;
    let diffs = read_curve(writer, pacer, gate, curve)?;
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
                getter: "GetFanIndexValue",
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
    apply_mode_with_gpu(writer, pacer, mode, fixed_raw, expert, GpuPower::Available)
}

pub fn apply_mode_with_gpu<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    mode: FanMode,
    fixed_raw: u8,
    expert: bool,
    gpu: GpuPower,
) -> Result<ApplyReport, ApplyError> {
    if mode == FanMode::Custom {
        return Err(ApplyError::CurveRequired);
    }
    let duty = match mode {
        FanMode::Max => 229,
        FanMode::Fixed => clamp_fixed_duty(fixed_raw, expert),
        _ => fixed_raw,
    };
    guarded(writer, pacer, gpu, |writer, pacer| {
        apply_mode_inner(writer, pacer, &mut Gate::new(gpu), Vec::new(), mode, duty)
    })
}

pub fn apply_fixed_duty<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    fixed_raw: u8,
    expert: bool,
) -> Result<ApplyReport, ApplyError> {
    apply_fixed_duty_with_gpu(writer, pacer, fixed_raw, expert, GpuPower::Available)
}

pub fn apply_fixed_duty_with_gpu<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    fixed_raw: u8,
    expert: bool,
    gpu: GpuPower,
) -> Result<ApplyReport, ApplyError> {
    let duty = clamp_fixed_duty(fixed_raw, expert);
    guarded(writer, pacer, gpu, |writer, pacer| {
        let mut gate = Gate::new(gpu);
        let mut warnings = Vec::new();
        run_script(
            writer,
            pacer,
            &mut gate,
            &fixed_duty_script(duty),
            &mut warnings,
        )?;
        let diffs = read_flags(writer, pacer, &mut gate, FanMode::Fixed, duty)?;
        if diffs.is_empty() {
            return Ok(ApplyReport {
                attempts: 1,
                warnings,
                restored_auto: false,
                diffs,
            });
        }
        warnings.push("fixed duty readback mismatched; running the full fixed script".to_string());
        apply_mode_inner(writer, pacer, &mut gate, warnings, FanMode::Fixed, duty)
    })
}

fn apply_mode_inner<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    gate: &mut Gate,
    mut warnings: Vec<String>,
    mode: FanMode,
    duty: u8,
) -> Result<ApplyReport, ApplyError> {
    let script = script_for(mode, duty);
    run_script(writer, pacer, gate, &script, &mut warnings)?;
    let mut diffs = read_flags(writer, pacer, gate, mode, duty)?;
    if diffs.is_empty() {
        return Ok(ApplyReport {
            attempts: 1,
            warnings,
            restored_auto: false,
            diffs,
        });
    }
    warnings.push("mode readback mismatched; retrying the full script once".to_string());
    run_script(writer, pacer, gate, &script, &mut warnings)?;
    diffs = read_flags(writer, pacer, gate, mode, duty)?;
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
    let (auto, fixed, step, speed) = match mode {
        FanMode::Gaming => (1, 0, 0, None),
        FanMode::Normal | FanMode::Quiet => (0, 0, 0, None),
        FanMode::Max | FanMode::Fixed => (0, 1, 1, Some(duty)),
        FanMode::Custom => (0, 0, 1, None),
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
    // GetNvPowerConfig exposes the transient TEMQ request, which may already
    // have reset to zero. Only durable fan/thermal flags establish this mode.
    check_flag(
        writer,
        pacer,
        gate,
        "GetNvThermalTarget",
        7,
        u8::from(mode == FanMode::Quiet),
        &mut diffs,
    )?;
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
    method: &'static str,
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
                    getter: method,
                    index,
                    expected_temp: 0,
                    expected_duty: expected,
                    actual_temp: None,
                    actual_duty: actual,
                });
            }
            Ok(())
        }
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
    if gate.gpu == GpuPower::Unavailable && write.method == "SetNvPowerConfig" {
        note_gpu_power_skipped(warnings);
        return Ok(());
    }
    gate.before(pacer);
    debug!(method = write.method, "WMI set");
    match writer.set(write.method, &write.params) {
        Ok(outcome) => {
            if let Some(message) = outcome.threw {
                // A consumed NVIDIA request has no durable success getter.
                // Legacy transports which return an exception inside Ok must
                // not turn that request into a claimed successful operation.
                if write.method == "SetNvPowerConfig" {
                    return Err(WmiError::CallFailed {
                        method: write.method.into(),
                        detail: message,
                    }
                    .into());
                }
                warnings.push(format!(
                    "{}: WMI returned an error ({message}); the call is kept and checked with a getter",
                    write.method
                ));
            }
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
    apply_profile_with_gpu(writer, pacer, profile, reason, GpuPower::Available)
}

pub fn apply_profile_with_gpu<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    profile: &Profile,
    reason: ApplyReason,
    gpu: GpuPower,
) -> Result<ApplyReport, ApplyError> {
    if profile.cpu_power_preset.is_some() {
        return Err(ApplyError::CpuBridgeRequired);
    }
    if let Some(limit) = profile.charge_limit {
        if !(60..=100).contains(&limit) {
            return Err(ApplyError::InvalidCharge(limit));
        }
    }
    if profile
        .charge_policy
        .is_some_and(|policy| policy != 0 && policy != 4)
        || (profile.charge_policy == Some(4) && profile.charge_limit.is_none())
        || (profile.charge_policy == Some(0) && profile.charge_limit.is_some())
    {
        return Err(ApplyError::InvalidChargePolicy);
    }
    for (field, value) in [
        ("nv_power_config", profile.nv_power_config),
        ("nv_thermal_target", profile.nv_thermal_target),
    ] {
        if let Some(value) = value.filter(|value| *value > 1) {
            return Err(ApplyError::InvalidSwitch { field, value });
        }
    }
    debug!(profile = %profile.name, ?reason, "apply profile");
    let mut report = match profile.fan_mode {
        FanMode::Custom => {
            let curve = profile.fan_curve().map_err(ApplyError::InvalidCurve)?;
            upload_curve_with_gpu(writer, pacer, &curve, gpu)?
        }
        mode => {
            let raw = profile
                .fixed_duty_raw
                .unwrap_or_else(|| crate::percent_to_raw(profile.fixed_duty_percent.unwrap_or(50)));
            apply_mode_with_gpu(writer, pacer, mode, raw, profile.expert_mode, gpu)?
        }
    };
    if profile.cpu_pl1_w.is_some() || profile.cpu_pl2_w.is_some() {
        report
            .warnings
            .push("Legacy CPU PL1/PL2 fields are stored only; select a CPU power preset in the desktop app".to_string());
    }
    guarded(writer, pacer, gpu, |writer, pacer| {
        if let Some(value) = profile.nv_thermal_target {
            pacer.pause();
            crate::set_nv_thermal_target(writer, pacer, value == 1)?;
        }
        if let Some(value) = profile.nv_power_config {
            if gpu == GpuPower::Available {
                pacer.pause();
                crate::set_nv_power_config(writer, pacer, value == 1)?;
            } else {
                note_gpu_power_skipped(&mut report.warnings);
            }
        }
        if profile.charge_policy == Some(0) {
            pacer.pause();
            crate::set_charge_policy(writer, pacer, crate::ChargePolicy::Standard)?;
        } else if let Some(limit) = profile.charge_limit {
            pacer.pause();
            crate::set_charge_policy(writer, pacer, crate::ChargePolicy::Custom(limit))?;
        }
        Ok(())
    })?;
    if profile.dynamic_boost.is_some() || profile.ai_boost.is_some() {
        report.warnings.push(
            "Dynamic Boost and AI Boost fields are stored only; they are not sent to WMI"
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
                getter: "GetFanIndexValue",
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
    use crate::preset::factory_curve;
    use aero_wmi::{CountPacer, MockAcpi, NoPacer};
    use std::panic::{catch_unwind, AssertUnwindSafe};

    fn sample() -> FanCurve {
        factory_curve()
    }

    fn methods(mock: &MockAcpi) -> Vec<String> {
        mock.calls.iter().map(|call| call.method.clone()).collect()
    }

    fn without_gpu() -> MockAcpi {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        mock.throw_setters.insert("SetNvPowerConfig".into());
        mock
    }

    fn assert_no_gpu_power_calls(mock: &MockAcpi) {
        assert_eq!(mock.calls_named("SetNvPowerConfig").count(), 0);
        assert_eq!(mock.calls_named("GetNvPowerConfig").count(), 0);
        assert_eq!(mock.calls_named("SetAIBoostStatus").count(), 0);
    }

    #[test]
    fn unavailable_gpu_preserves_fan_sequences_and_thermal_verification() {
        for mode in [
            FanMode::Normal,
            FanMode::Quiet,
            FanMode::Gaming,
            FanMode::Fixed,
            FanMode::Max,
        ] {
            let mut available = MockAcpi::aero_17_yb_fixture();
            apply_mode(&mut available, &mut NoPacer, mode, 120, false).unwrap();
            let mut explicit_available = MockAcpi::aero_17_yb_fixture();
            apply_mode_with_gpu(
                &mut explicit_available,
                &mut NoPacer,
                mode,
                120,
                false,
                GpuPower::Available,
            )
            .unwrap();
            assert_eq!(available.calls, explicit_available.calls);

            let mut unavailable = without_gpu();
            let mut pacer = CountPacer::default();
            let report = apply_mode_with_gpu(
                &mut unavailable,
                &mut pacer,
                mode,
                120,
                false,
                GpuPower::Unavailable,
            )
            .unwrap();
            let expected: Vec<_> = available
                .calls
                .into_iter()
                .filter(|call| call.method != "SetNvPowerConfig")
                .collect();
            assert_eq!(unavailable.calls, expected, "{mode:?}");
            assert_eq!(pacer.pauses, unavailable.calls.len() - 1);
            assert_eq!(report.warnings, [GPU_POWER_SKIPPED]);
            assert_eq!(
                unavailable.nv_thermal_target,
                u8::from(mode == FanMode::Quiet)
            );
            assert_eq!(unavailable.calls_named("GetNvThermalTarget").count(), 1);
            assert_no_gpu_power_calls(&unavailable);
        }
    }

    #[test]
    fn unavailable_gpu_supports_curve_uploads_edits_and_fixed_duty_fallback() {
        let mut mock = without_gpu();
        let report =
            upload_curve_with_gpu(&mut mock, &mut NoPacer, &sample(), GpuPower::Unavailable)
                .unwrap();
        assert_eq!(report.warnings, [GPU_POWER_SKIPPED]);
        assert_eq!(
            (mock.auto, mock.fixed, mock.step, mock.nv_thermal_target),
            (0, 0, 1, 0)
        );
        assert_eq!(mock.calls_named("SetNvThermalTarget").count(), 1);
        assert_eq!(mock.calls_named("GetFanIndexValue").count(), 15);
        mock.calls.clear();
        update_curve_with_gpu(&mut mock, &mut NoPacer, &sample(), GpuPower::Unavailable).unwrap();
        assert_eq!(mock.calls_named("SetFanIndexValue").count(), 15);
        assert_eq!(mock.calls_named("GetNvThermalTarget").count(), 1);
        assert_no_gpu_power_calls(&mock);

        // The fast duty path must keep the same GPU policy when it discovers
        // that the controller needs the full fixed-mode sequence instead.
        let report =
            apply_fixed_duty_with_gpu(&mut mock, &mut NoPacer, 120, false, GpuPower::Unavailable)
                .unwrap();
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning == GPU_POWER_SKIPPED));
        assert_eq!((mock.fixed_speed, mock.gpu_duty), (120, 120));
        assert_no_gpu_power_calls(&mock);
        mock.calls.clear();
        apply_fixed_duty_with_gpu(&mut mock, &mut NoPacer, 150, false, GpuPower::Unavailable)
            .unwrap();
        assert_eq!((mock.fixed_speed, mock.gpu_duty), (150, 150));
        assert_no_gpu_power_calls(&mock);

        // Curve edits also retain the policy when a full upload is needed.
        update_curve_with_gpu(&mut mock, &mut NoPacer, &sample(), GpuPower::Unavailable).unwrap();
        assert_eq!((mock.auto, mock.fixed, mock.step), (0, 0, 1));
        assert_no_gpu_power_calls(&mock);
    }

    #[test]
    fn unavailable_gpu_skips_profile_power_override_without_skipping_other_fields() {
        for name in ["Normal", "Gaming", "Deep Control"] {
            let mut profile = crate::builtin_config().profile(name).unwrap().clone();
            profile.nv_power_config = Some(1);
            profile.nv_thermal_target = Some(1);
            profile.charge_limit = Some(80);
            let mut mock = without_gpu();
            let report = apply_profile_with_gpu(
                &mut mock,
                &mut NoPacer,
                &profile,
                ApplyReason::Startup,
                GpuPower::Unavailable,
            )
            .unwrap();
            assert_eq!(report.warnings, [GPU_POWER_SKIPPED]);
            assert_eq!(mock.nv_thermal_target, 1);
            assert_eq!(mock.charge_stop, 80);
            assert!(mock.calls_named("GetNvThermalTarget").count() >= 1);
            assert_no_gpu_power_calls(&mock);
        }
    }

    #[test]
    fn unavailable_gpu_profile_recovery_preserves_policy_after_the_fan_apply() {
        let mut profile = crate::builtin_config().profile("Fixed").unwrap().clone();
        profile.nv_power_config = Some(1);
        profile.charge_limit = Some(80);
        for panic in [false, true] {
            let mut mock = without_gpu();
            if panic {
                mock.panic_on_set = Some("SetChargeStop".into());
            } else {
                mock.throw_setters.insert("SetChargeStop".into());
            }
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                apply_profile_with_gpu(
                    &mut mock,
                    &mut NoPacer,
                    &profile,
                    ApplyReason::Resume,
                    GpuPower::Unavailable,
                )
            }));
            if panic {
                assert!(outcome.is_err());
            } else {
                assert!(matches!(outcome.unwrap(), Err(ApplyError::Restored { .. })));
            }
            assert_eq!(
                (mock.auto, mock.fixed, mock.step, mock.nv_thermal_target),
                (0, 0, 0, 0)
            );
            assert_no_gpu_power_calls(&mock);
        }
    }

    #[test]
    fn unavailable_gpu_keeps_thermal_mismatch_and_restoration_checks() {
        let mut mock = without_gpu();
        mock.ignored_setters.insert("SetNvThermalTarget".into());
        let error = apply_mode_with_gpu(
            &mut mock,
            &mut NoPacer,
            FanMode::Quiet,
            0,
            false,
            GpuPower::Unavailable,
        )
        .unwrap_err();
        assert!(matches!(error, ApplyError::Restored { .. }));
        assert!(error.to_string().contains("GetNvThermalTarget"));
        assert_eq!(mock.calls_named("SetNvThermalTarget").count(), 3);
        assert_eq!(
            (mock.auto, mock.fixed, mock.step, mock.nv_thermal_target),
            (0, 0, 0, 0)
        );
        assert_no_gpu_power_calls(&mock);
    }

    #[test]
    fn unavailable_gpu_restores_after_curve_errors_and_panics() {
        for failure in ["setter", "readback", "panic"] {
            let mut mock = without_gpu();
            match failure {
                "setter" => {
                    mock.throw_setters.insert("SetFanIndexValue".into());
                }
                "readback" => mock.corrupt_fan_index_reads = 1_000,
                "panic" => mock.panic_on_set = Some("SetFanIndexValue".into()),
                _ => unreachable!(),
            }
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                upload_curve_with_gpu(&mut mock, &mut NoPacer, &sample(), GpuPower::Unavailable)
            }));
            if failure == "panic" {
                assert!(outcome.is_err());
            } else {
                assert!(matches!(outcome.unwrap(), Err(ApplyError::Restored { .. })));
            }
            assert_eq!(
                (mock.auto, mock.fixed, mock.step, mock.nv_thermal_target),
                (0, 0, 0, 0)
            );
            assert_no_gpu_power_calls(&mock);
        }
    }

    #[test]
    fn unavailable_gpu_restoration_does_not_depend_on_nvidia_power_requests() {
        let mut mock = without_gpu();
        mock.fixed = 1;
        mock.step = 1;
        mock.nv_thermal_target = 1;
        restore_firmware_auto_with_gpu(&mut mock, &mut NoPacer, GpuPower::Unavailable).unwrap();
        assert_eq!(
            (mock.auto, mock.fixed, mock.step, mock.nv_thermal_target),
            (0, 0, 0, 0)
        );
        assert_eq!(mock.calls_named("GetNvThermalTarget").count(), 1);
        assert_no_gpu_power_calls(&mock);
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
        assert!(slots.iter().all(|slot| *slot != (0, 0)));
        let expected = [
            (0, 57),
            (50, 68),
            (53, 80),
            (56, 91),
            (59, 103),
            (62, 114),
            (65, 125),
            (68, 137),
            (71, 148),
            (74, 160),
            (77, 171),
            (80, 183),
            (83, 194),
            (86, 206),
            (89, 229),
        ];
        assert_eq!(mock.curve, expected);
        assert_eq!(slots, expected);
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
    }

    #[test]
    fn mismatch_retries_once_then_restores_auto() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        mock.corrupt_fan_index_reads = 1_000;
        let err = upload_curve(&mut mock, &mut NoPacer, &sample()).unwrap_err();
        assert!(matches!(err, ApplyError::Restored { .. }));
        assert_eq!(mock.calls_named("SetFanIndexValue").count(), 30);
        assert_eq!(mock.auto, 0);
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
        assert_eq!(mock.curve[0], (0, percent_to_raw(25)));
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
    fn failed_setter_stops_upload_and_restores_firmware_mode() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        mock.throw_setters.insert("SetFanIndexValue".to_string());
        let err = upload_curve(&mut mock, &mut NoPacer, &sample()).unwrap_err();
        assert!(matches!(err, ApplyError::Restored { .. }));
        assert_eq!(mock.calls_named("SetFanIndexValue").count(), 1);
        assert_eq!((mock.fixed, mock.step), (0, 0));
    }

    #[test]
    fn failed_recovery_is_reported() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        mock.throw_setters.insert("SetFixedFanStatus".into());
        let err = upload_curve(&mut mock, &mut NoPacer, &sample()).unwrap_err();
        assert!(matches!(err, ApplyError::RestoreFailed { .. }));
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
        assert_eq!(mock.auto, 0);
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
    fn curve_edit_while_custom_writes_only_the_slots() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        upload_curve(&mut mock, &mut NoPacer, &sample()).unwrap();
        mock.calls.clear();
        let mut points = sample().points().to_vec();
        points[1].duty_raw += 5;
        let curve = FanCurve::try_new(points).unwrap();
        let report = update_curve(&mut mock, &mut NoPacer, &curve).unwrap();
        assert_eq!(report.attempts, 1);
        assert!(methods(&mock)
            .iter()
            .filter(|method| method.starts_with("Set"))
            .all(|method| method == "SetFanIndexValue"));
        assert_eq!(mock.calls_named("SetFanIndexValue").count(), 15);
    }

    #[test]
    fn curve_edit_outside_custom_falls_back_to_the_full_upload() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        let mut pacer = CountPacer::default();
        let report = update_curve(&mut mock, &mut pacer, &sample()).unwrap();
        assert!(report.warnings.iter().any(|w| w.contains("full upload")));
        assert_eq!(mock.calls_named("SetStepFanStatus").count(), 1);
        assert_eq!((mock.auto, mock.step), (0, 1));
        assert_eq!(pacer.pauses, mock.calls.len() - 1);
    }

    #[test]
    fn duty_change_in_fixed_mode_writes_only_the_two_speeds() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        apply_mode(&mut mock, &mut NoPacer, FanMode::Fixed, 120, false).unwrap();
        mock.calls.clear();
        let report = apply_fixed_duty(&mut mock, &mut NoPacer, 150, false).unwrap();
        assert_eq!(report.attempts, 1);
        let sets: Vec<_> = methods(&mock)
            .into_iter()
            .filter(|method| method.starts_with("Set"))
            .collect();
        assert_eq!(sets, ["SetFixedFanSpeed", "SetGPUFanDuty"]);
        assert_eq!(mock.fixed_speed, 150);
        assert_eq!(mock.gpu_duty, 150);
    }

    #[test]
    fn duty_change_outside_fixed_mode_falls_back_to_the_full_script() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        let mut pacer = CountPacer::default();
        let report = apply_fixed_duty(&mut mock, &mut pacer, 10, false).unwrap();
        assert_eq!(report.attempts, 1);
        assert!(report
            .warnings
            .iter()
            .any(|w| w.contains("full fixed script")));
        assert_eq!(mock.calls_named("SetFixedFanStatus").count(), 2);
        assert_eq!((mock.auto, mock.fixed, mock.fixed_speed), (0, 1, 80));
        assert_eq!(pacer.pauses, mock.calls.len() - 1);
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
    fn modes_leave_the_firmware_in_the_control_center_state() {
        let cases = [
            (FanMode::Normal, 0, 0, 0),
            (FanMode::Gaming, 1, 0, 1),
            (FanMode::Quiet, 0, 1, 0),
        ];
        for (mode, auto, thermal, nv) in cases {
            let mut mock = MockAcpi::aero_17_yb_fixture();
            let report = apply_mode(&mut mock, &mut NoPacer, mode, 0, false).unwrap();
            assert_eq!(
                (mock.auto, mock.nv_thermal_target, mock.nv_power_config),
                (auto, thermal, nv),
                "{mode:?}"
            );
            assert_eq!((mock.fixed, mock.step, mock.super_quiet), (0, 0, 0));
            assert!(
                report.warnings.is_empty(),
                "{mode:?}: {:?}",
                report.warnings
            );
            assert_eq!(mock.calls_named("SetFanIndexValue").count(), 0);
        }
    }

    #[test]
    fn transitions_clear_quiet_and_gaming_power_settings() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        apply_mode(&mut mock, &mut NoPacer, FanMode::Quiet, 0, false).unwrap();
        apply_mode(&mut mock, &mut NoPacer, FanMode::Gaming, 0, false).unwrap();
        assert_eq!((mock.nv_thermal_target, mock.nv_power_config), (0, 1));
        apply_mode(&mut mock, &mut NoPacer, FanMode::Fixed, 120, false).unwrap();
        assert_eq!((mock.nv_thermal_target, mock.nv_power_config), (0, 0));
        apply_mode(&mut mock, &mut NoPacer, FanMode::Gaming, 0, false).unwrap();
        upload_curve(&mut mock, &mut NoPacer, &sample()).unwrap();
        assert_eq!((mock.nv_thermal_target, mock.nv_power_config), (0, 0));
    }

    #[test]
    fn profile_gpu_power_fields_override_the_mode_defaults() {
        let mut profile = crate::builtin_config().profile("Normal").unwrap().clone();
        profile.fan_mode = FanMode::Gaming;
        profile.nv_power_config = Some(0);
        profile.nv_thermal_target = Some(1);
        let mut mock = MockAcpi::aero_17_yb_fixture();
        apply_profile(&mut mock, &mut NoPacer, &profile, ApplyReason::User).unwrap();
        assert_eq!(mock.auto, 1);
        assert_eq!(mock.nv_power_config, 0);
        assert_eq!(mock.nv_thermal_target, 1);
    }

    #[test]
    fn profile_thermal_override_noop_is_rejected_and_recovers_normal() {
        let mut profile = crate::builtin_config().profile("Normal").unwrap().clone();
        profile.nv_thermal_target = Some(1);
        let mut mock = MockAcpi::aero_17_yb_fixture();
        mock.ignored_setters.insert("SetNvThermalTarget".into());
        let result = apply_profile(&mut mock, &mut NoPacer, &profile, ApplyReason::User);
        assert!(matches!(result, Err(ApplyError::Restored { .. })));
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("GetNvThermalTarget"));
        assert_eq!(
            (mock.auto, mock.fixed, mock.step, mock.nv_thermal_target),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn durable_flag_mismatches_name_the_getter_then_restore_normal() {
        for (mode, setter, getter) in [
            (FanMode::Gaming, "SetAutoFanStatus", "GetAutoFanStatus"),
            (FanMode::Quiet, "SetNvThermalTarget", "GetNvThermalTarget"),
        ] {
            let mut mock = MockAcpi::aero_17_yb_fixture();
            mock.auto = 0;
            mock.ignored_setters.insert(setter.into());
            let error = apply_mode(&mut mock, &mut NoPacer, mode, 0, false).unwrap_err();
            assert!(matches!(error, ApplyError::Restored { .. }));
            assert!(error.to_string().contains(getter), "{error}");
            assert_eq!(
                mock.calls_named(setter).count(),
                3,
                "two attempts and Normal recovery"
            );
            assert_eq!(mock.calls_named(getter).count(), 3);
            assert_eq!(
                (
                    mock.auto,
                    mock.fixed,
                    mock.step,
                    mock.nv_power_config,
                    mock.nv_thermal_target
                ),
                (0, 0, 0, 0, 0)
            );
        }
    }

    #[test]
    fn gaming_does_not_rollback_when_the_gpu_request_has_been_consumed() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        mock.auto = 0;
        mock.consume_nv_power_requests = true;
        let report = apply_mode(&mut mock, &mut NoPacer, FanMode::Gaming, 0, false).unwrap();
        assert_eq!(report.attempts, 1);
        assert_eq!(
            mock.get("GetNvPowerConfig", &[])
                .unwrap()
                .get("Data")
                .unwrap()
                .as_u32(),
            Some(0)
        );
        assert_eq!((mock.auto, mock.fixed, mock.step), (1, 0, 0));
        assert_eq!(mock.calls_named("SetNvPowerConfig").count(), 1);
        assert_eq!(mock.calls_named("SetAutoFanStatus").count(), 1);
    }

    #[test]
    fn normal_recovery_does_not_claim_success_when_thermal_clear_is_ignored() {
        let mut mock = MockAcpi::aero_17_yb_fixture();
        apply_mode(&mut mock, &mut NoPacer, FanMode::Quiet, 0, false).unwrap();
        mock.ignored_setters.insert("SetNvThermalTarget".into());
        assert!(matches!(
            apply_mode(&mut mock, &mut NoPacer, FanMode::Normal, 0, false),
            Err(ApplyError::RestoreFailed { .. })
        ));
        assert_eq!(mock.nv_thermal_target, 1);
        assert_eq!((mock.auto, mock.fixed, mock.step), (0, 0, 0));
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
    fn profiles_restore_standard_charging_after_a_custom_limit() {
        let mut profile = crate::builtin_config().profile("Normal").unwrap().clone();
        let mut mock = MockAcpi::aero_17_yb_fixture();
        profile.charge_policy = Some(4);
        profile.charge_limit = Some(80);
        apply_profile(&mut mock, &mut NoPacer, &profile, ApplyReason::User).unwrap();
        assert_eq!((mock.charge_policy, mock.charge_stop), (4, 80));
        profile.charge_policy = Some(0);
        profile.charge_limit = None;
        apply_profile(&mut mock, &mut NoPacer, &profile, ApplyReason::User).unwrap();
        assert_eq!(mock.charge_policy, 0);
    }

    #[test]
    fn failed_profile_charge_write_recovers_fixed_fans() {
        let mut profile = crate::builtin_config().profile("Fixed").unwrap().clone();
        profile.charge_limit = Some(80);
        let mut mock = MockAcpi::aero_17_yb_fixture();
        mock.throw_setters.insert("SetChargeStop".into());
        assert!(matches!(
            apply_profile(&mut mock, &mut NoPacer, &profile, ApplyReason::User),
            Err(ApplyError::Restored { .. })
        ));
        assert_eq!((mock.fixed, mock.step), (0, 0));
    }

    #[test]
    fn invalid_profile_switch_is_rejected_before_any_write() {
        let mut profile = crate::builtin_config().profile("Normal").unwrap().clone();
        profile.nv_power_config = Some(2);
        let mut mock = MockAcpi::aero_17_yb_fixture();
        assert!(matches!(
            apply_profile(&mut mock, &mut NoPacer, &profile, ApplyReason::User),
            Err(ApplyError::InvalidSwitch { .. })
        ));
        assert!(mock.calls.is_empty());
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
        assert_eq!(mock.nv_thermal_target, 1);
        assert_eq!(mock.nv_power_config, 0);
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
            .any(|warning| warning.contains("Legacy CPU")));
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("not sent")));
        assert_eq!(mock.calls_named("SetDynamicBoostStatus").count(), 0);
    }

    #[test]
    fn cpu_preset_requires_a_bridge_before_any_wmi_write() {
        let mut profile = crate::builtin_config().profile("Gaming").unwrap().clone();
        profile.cpu_power_preset = Some(crate::CpuPowerPreset::Level5);
        let mut mock = MockAcpi::aero_17_yb_fixture();
        assert!(matches!(
            apply_profile(&mut mock, &mut NoPacer, &profile, ApplyReason::User),
            Err(ApplyError::CpuBridgeRequired)
        ));
        assert!(mock.calls.is_empty());
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
        apply_mode(&mut mock, &mut NoPacer, FanMode::Gaming, 0, false).unwrap();
        let call = mock.calls_named("SetAutoFanStatus").last().unwrap();
        assert_eq!(call.params, vec![("Data".to_string(), 1)]);
    }
}
