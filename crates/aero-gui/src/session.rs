//! Owns the embedded-controller connection on one thread.
//!
//! `WindowsWriter` is tied to the COM apartment that created it, so the UI
//! thread only sends commands. Sensor polls are getters and do not set fan duty.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use aero_core::{
    apply_fixed_duty_with_gpu, apply_mode_with_gpu, apply_profile_with_gpu, clamp_fixed_duty,
    restore_firmware_auto_with_gpu, set_charge_policy, set_nv_power_config, set_nv_thermal_target,
    update_curve_with_gpu, upload_curve_with_gpu, ApplyError, ApplyReason, ChargePolicy,
    CurvePoint, FanCurve, FanMode, GpuPower, Profile,
};
#[cfg(windows)]
use aero_wmi::FixedPacer;
use aero_wmi::{
    decode_rpm, AcpiReader, AcpiWriter, ClassStatus, MockAcpi, NoPacer, Pacer, RpmOrder,
    SetOutcome, SkuInfo, SkuSource, WmiError, WmiOutput, WriteParams,
};

use crate::gpu::GpuStatus;
use crate::log_file::{format_call, CallLog};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendChoice {
    Mock,
    /// Real `GB_WMIACPI` connection. Writes require an elevated token.
    Real {
        elevated: bool,
    },
}

impl BackendChoice {
    fn label(self) -> &'static str {
        match self {
            Self::Mock => "MOCK",
            Self::Real { .. } => "REAL",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SensorSnapshot {
    pub gpu_status: GpuStatus,
    pub cpu: String,
    pub gpu: String,
    pub gpu2: String,
    pub rpm1: String,
    pub rpm2: String,
    pub flags: String,
    pub error: Option<String>,
    /// Read at startup and after a battery or power write, not on every poll.
    pub power: Option<PowerSnapshot>,
}

/// Raw getter values, or the error text.
#[derive(Clone, Debug)]
pub struct PowerSnapshot {
    pub charge_policy: Result<u32, String>,
    pub charge_stop: Result<u32, String>,
    pub battery_health: Result<u32, String>,
    pub nv_power_config: Result<u32, String>,
    pub nv_thermal_target: Result<u32, String>,
    pub camera: Result<u32, String>,
    pub sleep_usb_charge: Result<u32, String>,
    pub hibernation_usb_charge: Result<u32, String>,
    pub identity: Result<SkuInfo, String>,
    pub fan_state: Result<crate::fan_state::RecognizedFanState, crate::fan_state::FanStateError>,
}

#[derive(Clone, Copy, Debug)]
pub enum PowerWrite {
    Charge(ChargePolicy),
    NvPowerConfig(bool),
    ThermalTarget(bool),
    Camera(bool),
    SleepUsbCharge(bool),
    HibernationUsbCharge(bool),
    DisplayOff,
}

#[derive(Clone, Debug)]
pub struct SessionEvent {
    pub sensors: Option<SensorSnapshot>,
    pub message: Option<String>,
    pub error: bool,
    pub busy: bool,
    /// Present only when a requested fan apply has finished verification or failed.
    pub fan_result: Option<bool>,
    /// Last GPU request sent by this operation, never a measured power budget.
    pub gpu_request: Option<Result<bool, String>>,
    pub cpu_power: Option<Result<crate::cpu_power::CpuPowerSnapshot, String>>,
}

enum Command {
    Profile(Profile, ApplyReason),
    Apply {
        mode: FanMode,
        raw: u8,
        expert: bool,
    },
    FixedDuty {
        raw: u8,
        expert: bool,
    },
    Upload(Vec<CurvePoint>),
    UpdateCurve(Vec<CurvePoint>),
    MaxTest,
    Power(PowerWrite),
    RefreshPower,
    CpuPower(Option<aero_core::CpuPowerPreset>),
    ModelConfirmation(bool),
    #[cfg(test)]
    MockGpuStatus(GpuStatus),
    Shutdown,
}

pub struct FanSession {
    tx: Sender<Command>,
    rx: Receiver<SessionEvent>,
    join: Option<JoinHandle<()>>,
    pub backend_label: String,
    pub writes_enabled: bool,
    pub log_path: String,
    pub banner: Option<String>,
    pub initial_sensors: Option<SensorSnapshot>,
}

impl FanSession {
    pub fn profile(&self, profile: Profile, reason: ApplyReason) {
        let _ = self.tx.send(Command::Profile(profile, reason));
    }
    pub fn start(choice: BackendChoice) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let join = thread::spawn(move || worker(choice, cmd_rx, ev_tx));
        let hello = ev_rx.recv().unwrap_or_else(|_| SessionEvent {
            sensors: None,
            message: Some(format!(
                "{}|0||The fan thread stopped before it connected.",
                choice.label()
            )),
            error: true,
            busy: false,
            fan_result: None,
            gpu_request: None,
            cpu_power: None,
        });
        let (backend_label, writes_enabled, log_path, banner) = parse_hello(&hello);
        Self {
            tx: cmd_tx,
            rx: ev_rx,
            join: Some(join),
            backend_label,
            writes_enabled,
            log_path,
            banner,
            initial_sensors: hello.sensors,
        }
    }

    pub fn poll(&self) -> Option<SessionEvent> {
        self.rx.try_recv().ok()
    }

    pub fn apply(&self, mode: FanMode, raw: u8, expert: bool) {
        let _ = self.tx.send(Command::Apply { mode, raw, expert });
    }

    pub fn fixed_duty(&self, raw: u8, expert: bool) {
        let _ = self.tx.send(Command::FixedDuty { raw, expert });
    }

    pub fn upload(&self, points: Vec<CurvePoint>) {
        let _ = self.tx.send(Command::Upload(points));
    }

    pub fn update_curve(&self, points: Vec<CurvePoint>) {
        let _ = self.tx.send(Command::UpdateCurve(points));
    }

    pub fn max_test(&self) {
        let _ = self.tx.send(Command::MaxTest);
    }

    pub fn power(&self, write: PowerWrite) {
        let _ = self.tx.send(Command::Power(write));
    }

    pub fn cpu_power(&self, preset: Option<aero_core::CpuPowerPreset>) {
        let _ = self.tx.send(Command::CpuPower(preset));
    }

    pub fn refresh_power(&self) {
        let _ = self.tx.send(Command::RefreshPower);
    }

    pub fn set_model_confirmation(&self, confirmed: bool) {
        let _ = self.tx.send(Command::ModelConfirmation(confirmed));
    }

    #[cfg(test)]
    pub fn set_mock_gpu_status(&self, status: GpuStatus) {
        let _ = self.tx.send(Command::MockGpuStatus(status));
    }
}

impl Drop for FanSession {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

fn parse_hello(event: &SessionEvent) -> (String, bool, String, Option<String>) {
    let text = event.message.clone().unwrap_or_default();
    // "REAL|MOCK|<writes 0/1>|<log path>|<banner>"
    let mut parts = text.splitn(5, '|');
    let label = parts.next().unwrap_or("MOCK").to_string();
    let explicit_writes = parts.next().is_some_and(|value| value == "1");
    let writes = matches!(label.as_str(), "REAL" | "MOCK") && explicit_writes;
    let log_path = parts.next().unwrap_or("").to_string();
    let banner = parts
        .next()
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    (
        label,
        writes,
        log_path,
        banner.or_else(|| event.error.then(|| text.clone())),
    )
}

fn worker(choice: BackendChoice, rx: Receiver<Command>, tx: Sender<SessionEvent>) {
    let log = CallLog::open().ok();
    let log_path = log
        .as_ref()
        .map(|log| log.path().display().to_string())
        .unwrap_or_else(|| "(log unavailable)".to_string());
    let mut live = match open_backend(choice, log) {
        Ok(live) => live,
        Err(err) => {
            let _ = tx.send(SessionEvent {
                sensors: None,
                message: Some(format!("{}|0|{log_path}|{err}", choice.label())),
                error: true,
                busy: false,
                fan_result: None,
                gpu_request: None,
                cpu_power: None,
            });
            return;
        }
    };
    let log_path = match &live {
        Live::Mock(backend) => backend.log.path().display().to_string(),
        #[cfg(windows)]
        Live::Read(backend) => backend.log.path().display().to_string(),
        #[cfg(windows)]
        Live::Write(backend) => backend.log.path().display().to_string(),
    };
    live.refresh_gpu_status();
    let _ = tx.send(SessionEvent {
        sensors: read_sensors_and_power(&mut live),
        message: Some(hello_line(&live, &log_path)),
        error: false,
        busy: false,
        fan_result: None,
        gpu_request: None,
        cpu_power: None,
    });
    let identity = crate::device_identity::read(matches!(choice, BackendChoice::Mock));
    let mut cpu = crate::cpu_power::CpuPower::new(
        matches!(choice, BackendChoice::Mock),
        crate::device_identity::has_yb_command_set(&identity),
        matches!(choice, BackendChoice::Real { elevated: true }),
    );
    let mut restore_on_exit = false;
    loop {
        let command = rx.recv_timeout(Duration::from_secs(2));
        // Refresh before an operation, never part-way through its verification
        // or recovery. PnP detection does not open the NVIDIA driver.
        live.refresh_gpu_status();
        match command {
            Ok(Command::Profile(mut profile, reason)) => {
                let _ = tx.send(busy());
                if let Err(error) = crate::profile_store::validate_profile(&profile) {
                    let _ = tx.send(SessionEvent {
                        message: Some(error),
                        error: true,
                        fan_result: Some(false),
                        gpu_request: None,
                        ..idle()
                    });
                    continue;
                }
                // CPU is preflighted and verified before any profile WMI writes.
                // If it fails, keep the existing fan/GPU state unchanged.
                let requested_gpu = profile
                    .nv_power_config
                    .map(|value| value == 1)
                    .unwrap_or(profile.fan_mode == FanMode::Gaming);
                let preset = profile.cpu_power_preset.take();
                let mut cpu_snapshot = None;
                if let Some(preset) = preset {
                    let outcome = cpu.apply(preset);
                    if let Err(error) = &outcome {
                        let _ = tx.send(SessionEvent {
                            message: Some(format!("Profile was not applied: {error}")),
                            error: true,
                            fan_result: Some(false),
                            gpu_request: None,
                            cpu_power: Some(outcome),
                            ..idle()
                        });
                        continue;
                    }
                    cpu_snapshot = Some(outcome);
                }
                // CPU preflight can take time. Recheck availability immediately
                // before the fan sequence, then freeze it through verification.
                live.refresh_gpu_status();
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    let gpu = live.gpu_power();
                    let report = match &mut live {
                        Live::Mock(backend) => {
                            apply_profile_with_gpu(backend, &mut NoPacer, &profile, reason, gpu)?
                        }
                        #[cfg(windows)]
                        Live::Write(backend) => apply_profile_with_gpu(
                            backend,
                            &mut FixedPacer::standard(),
                            &profile,
                            reason,
                            gpu,
                        )?,
                        #[cfg(windows)]
                        Live::Read(_) => {
                            return Err(ApplyError::Wmi(WmiError::NotAvailable(
                                "profile writes need elevation",
                            )))
                        }
                    };
                    Ok(format!(
                        "Applied {}. {}",
                        profile.name,
                        report.warnings.join(" ")
                    ))
                }));
                restore_on_exit = matches!(profile.fan_mode, FanMode::Fixed | FanMode::Max);
                let mut event = finish_fan_apply(
                    &mut live,
                    &mut restore_on_exit,
                    outcome,
                    Some(requested_gpu),
                );
                if event.fan_result == Some(false) && preset.is_some() {
                    // CPU writes are a separate subsystem. Do not claim a full
                    // rollback when the fan/GPU portion of the profile failed.
                    event.message.get_or_insert_with(String::new).push_str(
                        " CPU power was applied separately and remains at the requested preset.",
                    );
                }
                event.cpu_power = cpu_snapshot;
                let _ = tx.send(event);
            }
            Ok(Command::CpuPower(preset)) => {
                let _ = tx.send(busy());
                let outcome = if let Some(preset) = preset {
                    cpu.apply(preset)
                } else {
                    cpu.read()
                };
                let (message, error) = match &outcome {
                    Ok(snapshot) => (
                        format!(
                            "CPU power confirmed: sustained {:.2} W, short burst {:.2} W.",
                            snapshot.pl1_mw as f64 / 1000.0,
                            snapshot.pl2_mw as f64 / 1000.0
                        ),
                        false,
                    ),
                    Err(error) => (error.clone(), true),
                };
                let _ = tx.send(SessionEvent {
                    cpu_power: Some(outcome),
                    message: Some(message),
                    error,
                    ..idle()
                });
            }
            Ok(Command::ModelConfirmation(confirmed)) => {
                let mock = matches!(choice, BackendChoice::Mock);
                let identity = crate::device_identity::read(mock);
                cpu.set_supported_model(crate::device_identity::supports_yb(&identity, confirmed));
            }
            #[cfg(test)]
            Ok(Command::MockGpuStatus(status)) => {
                if let Live::Mock(backend) = &mut live {
                    backend.gpu_status = status;
                }
                let _ = tx.send(SessionEvent {
                    sensors: read_sensors_and_power(&mut live),
                    ..idle()
                });
            }
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                if restore_on_exit {
                    let _ = safe_restore(&mut live);
                }
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = tx.send(SessionEvent {
                    sensors: read_sensors(&mut live),
                    message: None,
                    error: false,
                    busy: false,
                    fan_result: None,
                    gpu_request: None,
                    cpu_power: None,
                });
            }
            Ok(Command::Apply { mode, raw, expert }) => {
                let _ = tx.send(busy());
                let outcome =
                    catch_unwind(AssertUnwindSafe(|| apply_any(&mut live, mode, raw, expert)));
                restore_on_exit = matches!(mode, FanMode::Fixed | FanMode::Max);
                let _ = tx.send(finish_fan_apply(
                    &mut live,
                    &mut restore_on_exit,
                    outcome,
                    Some(mode == FanMode::Gaming),
                ));
            }
            Ok(Command::FixedDuty { raw, expert }) => {
                let _ = tx.send(busy());
                let outcome =
                    catch_unwind(AssertUnwindSafe(|| fixed_duty_any(&mut live, raw, expert)));
                restore_on_exit = true;
                let _ = tx.send(finish_fan_apply(
                    &mut live,
                    &mut restore_on_exit,
                    outcome,
                    None,
                ));
            }
            Ok(Command::Upload(points)) => {
                let _ = tx.send(busy());
                let outcome =
                    catch_unwind(AssertUnwindSafe(|| upload_any(&mut live, points, false)));
                restore_on_exit = false;
                let _ = tx.send(finish_fan_apply(
                    &mut live,
                    &mut restore_on_exit,
                    outcome,
                    Some(false),
                ));
            }
            Ok(Command::UpdateCurve(points)) => {
                let _ = tx.send(busy());
                let outcome =
                    catch_unwind(AssertUnwindSafe(|| upload_any(&mut live, points, true)));
                restore_on_exit = false;
                let _ = tx.send(finish_fan_apply(
                    &mut live,
                    &mut restore_on_exit,
                    outcome,
                    None,
                ));
            }
            Ok(Command::Power(write)) => {
                let _ = tx.send(busy());
                let outcome = catch_unwind(AssertUnwindSafe(|| power_any(&mut live, write)));
                let (message, error) = match outcome {
                    Ok(Ok(text)) => (text, false),
                    Ok(Err(err)) => (err.to_string(), true),
                    Err(_) => ("The power command panicked.".to_string(), true),
                };
                let gpu_request = match write {
                    PowerWrite::NvPowerConfig(on) => {
                        Some(if error { Err(message.clone()) } else { Ok(on) })
                    }
                    _ => None,
                };
                let _ = tx.send(SessionEvent {
                    sensors: read_sensors_and_power(&mut live),
                    message: Some(message),
                    error,
                    busy: false,
                    fan_result: None,
                    gpu_request,
                    cpu_power: None,
                });
            }
            Ok(Command::RefreshPower) => {
                let _ = tx.send(SessionEvent {
                    message: Some("Reading firmware settings…".into()),
                    ..busy()
                });
                let event =
                    match catch_unwind(AssertUnwindSafe(|| read_sensors_and_power(&mut live))) {
                        Ok(sensors) => SessionEvent {
                            sensors,
                            message: Some("Firmware reads finished.".into()),
                            error: false,
                            busy: false,
                            fan_result: None,
                            gpu_request: None,
                            cpu_power: None,
                        },
                        Err(_) => SessionEvent {
                            sensors: None,
                            message: Some(
                                "Firmware refresh panicked; readings are unavailable.".into(),
                            ),
                            error: true,
                            busy: false,
                            fan_result: None,
                            gpu_request: None,
                            cpu_power: None,
                        },
                    };
                let _ = tx.send(event);
            }
            Ok(Command::MaxTest) => {
                let _ = tx.send(SessionEvent {
                    sensors: None,
                    message: Some("Max for 10 seconds, then firmware auto.".to_string()),
                    error: false,
                    busy: true,
                    fan_result: None,
                    gpu_request: None,
                    cpu_power: None,
                });
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    apply_any(&mut live, FanMode::Max, 229, false)
                }));
                match outcome {
                    Ok(Ok(text)) => {
                        let _ = tx.send(report_from("Max", Ok(text)));
                    }
                    other => {
                        let _ = tx.send(finish_fan_apply(
                            &mut live,
                            &mut restore_on_exit,
                            other,
                            None,
                        ));
                        continue;
                    }
                }
                let shutdown = wait_or_shutdown(&rx, &tx, &mut live, Duration::from_secs(10));
                // The adapter may have been disabled during the test. Recovery
                // is a new sequence and must use its current availability.
                live.refresh_gpu_status();
                if shutdown {
                    let _ = restore_any(&mut live);
                    break;
                }
                let restored = catch_unwind(AssertUnwindSafe(|| restore_any(&mut live)));
                let result = match restored {
                    Ok(result) => result.map_err(|error| error.to_string()),
                    Err(_) => Err("Restore after the Max test panicked.".to_string()),
                };
                let failed = result.is_err();
                restore_on_exit = failed;
                let _ = tx.send(SessionEvent {
                    sensors: read_sensors_and_power(&mut live),
                    message: Some(result.map_or_else(
                        |error| format!("Max test recovery failed: {error}"),
                        |_| "Max test finished. Normal mode restored.".into(),
                    )),
                    error: failed,
                    busy: false,
                    fan_result: Some(!failed),
                    gpu_request: live.gpu_status().allows_commands().then(|| {
                        if failed {
                            Err("Max recovery failed.".into())
                        } else {
                            Ok(false)
                        }
                    }),
                    cpu_power: None,
                });
            }
        }
    }
}

fn wait_or_shutdown(
    rx: &Receiver<Command>,
    tx: &Sender<SessionEvent>,
    live: &mut Live,
    duration: Duration,
) -> bool {
    let end = Instant::now() + duration;
    while Instant::now() < end {
        let slice = (end - Instant::now()).min(Duration::from_secs(2));
        match rx.recv_timeout(slice) {
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => return true,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = tx.send(SessionEvent {
                    sensors: read_sensors(live),
                    message: None,
                    error: false,
                    busy: true,
                    fan_result: None,
                    gpu_request: None,
                    cpu_power: None,
                });
            }
            Ok(_) => {
                let _ = tx.send(SessionEvent {
                    sensors: None,
                    message: Some(
                        "Ignored a fan command during the 10 second Max test.".to_string(),
                    ),
                    error: true,
                    busy: true,
                    fan_result: None,
                    gpu_request: None,
                    cpu_power: None,
                });
            }
        }
    }
    false
}

fn idle() -> SessionEvent {
    SessionEvent {
        sensors: None,
        message: None,
        error: false,
        busy: false,
        fan_result: None,
        gpu_request: None,
        cpu_power: None,
    }
}

#[cfg(test)]
mod cpu_profile_tests {
    use super::*;

    fn completed(session: &FanSession) -> SessionEvent {
        loop {
            let event = session
                .rx
                .recv_timeout(Duration::from_secs(3))
                .expect("mock command completes");
            if !event.busy {
                return event;
            }
        }
    }

    #[test]
    fn gpu_request_and_gaming_survive_a_cleared_request_flag_and_refresh() {
        let session = FanSession::start(BackendChoice::Mock);
        session.power(PowerWrite::NvPowerConfig(true));
        let event = completed(&session);
        assert_eq!(event.gpu_request, Some(Ok(true)));
        assert_eq!(event.fan_result, None);
        assert!(!event.error);
        assert_eq!(event.sensors.unwrap().power.unwrap().nv_power_config, Ok(0));
        session.apply(FanMode::Gaming, 0, false);
        let event = completed(&session);
        assert_eq!(event.gpu_request, Some(Ok(true)));
        assert_eq!(event.fan_result, Some(true));
        assert_eq!(
            event
                .sensors
                .unwrap()
                .power
                .unwrap()
                .fan_state
                .unwrap()
                .mode,
            FanMode::Gaming
        );
        session.refresh_power();
        let event = completed(&session);
        assert_eq!(event.gpu_request, None);
        let power = event.sensors.unwrap().power.unwrap();
        assert_eq!(power.nv_power_config, Ok(0));
        assert_eq!(power.fan_state.unwrap().mode, FanMode::Gaming);
    }

    #[test]
    fn cpu_commands_do_not_report_a_fan_change_and_profiles_keep_cpu_readback() {
        let session = FanSession::start(BackendChoice::Mock);
        session.cpu_power(None);
        let initial = completed(&session);
        assert_eq!(initial.fan_result, None);
        assert_eq!(initial.cpu_power.unwrap().unwrap().pl1_mw, 52_000);
        session.cpu_power(Some(aero_core::CpuPowerPreset::Battery));
        let applied = completed(&session);
        assert_eq!(applied.fan_result, None);
        assert_eq!(applied.cpu_power.unwrap().unwrap().pl2_mw, 56_250);
        let mut profile = aero_core::builtin_config()
            .profile("Gaming")
            .unwrap()
            .clone();
        profile.cpu_power_preset = Some(aero_core::CpuPowerPreset::Level5);
        session.profile(profile, ApplyReason::User);
        let event = completed(&session);
        assert_eq!(event.fan_result, Some(true));
        assert_eq!(event.cpu_power.unwrap().unwrap().pl1_mw, 62_000);
        assert_eq!(event.gpu_request, Some(Ok(true)));
        assert_eq!(event.sensors.unwrap().power.unwrap().nv_power_config, Ok(0));
    }

    #[test]
    fn invalid_profile_does_not_apply_cpu_power() {
        let session = FanSession::start(BackendChoice::Mock);
        let mut profile = aero_core::builtin_config()
            .profile("Gaming")
            .unwrap()
            .clone();
        profile.cpu_power_preset = Some(aero_core::CpuPowerPreset::Level5);
        profile.charge_limit = Some(101);
        session.profile(profile, ApplyReason::User);
        assert_eq!(completed(&session).fan_result, Some(false));
        session.cpu_power(None);
        assert_eq!(
            completed(&session).cpu_power.unwrap().unwrap().pl1_mw,
            52_000
        );
    }

    #[test]
    fn model_confirmation_changes_keep_current_cpu_limits() {
        let session = FanSession::start(BackendChoice::Mock);
        session.cpu_power(Some(aero_core::CpuPowerPreset::Level5));
        assert_eq!(
            completed(&session).cpu_power.unwrap().unwrap().pl1_mw,
            62_000
        );
        session.set_model_confirmation(true);
        session.set_model_confirmation(false);
        session.cpu_power(None);
        assert_eq!(
            completed(&session).cpu_power.unwrap().unwrap().pl1_mw,
            62_000
        );
    }
}

fn busy() -> SessionEvent {
    SessionEvent {
        sensors: None,
        message: Some("Sending the firmware sequence…".to_string()),
        error: false,
        busy: true,
        fan_result: None,
        gpu_request: None,
        cpu_power: None,
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;

    #[test]
    fn failed_or_malformed_handshake_never_enables_writes() {
        for message in [
            "worker stopped",
            "REAL",
            "REAL||",
            "OTHER|1|log|",
            "MOCK|0||",
        ] {
            let event = SessionEvent {
                sensors: None,
                message: Some(message.into()),
                error: true,
                busy: false,
                fan_result: None,
                gpu_request: None,
                cpu_power: None,
            };
            assert!(!parse_hello(&event).1, "{message}");
        }
        assert_eq!(BackendChoice::Real { elevated: true }.label(), "REAL");
        assert_eq!(BackendChoice::Mock.label(), "MOCK");
    }

    #[test]
    fn progress_event_is_not_a_fan_completion() {
        assert_eq!(busy().fan_result, None);
    }

    #[test]
    fn fan_completion_reports_success_and_failure_explicitly() {
        let dir = std::env::temp_dir().join(format!(
            "aero-completion-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let log = CallLog::open_in(&dir).unwrap();
        let mut live = Live::Mock(Logged::new(MockAcpi::aero_17_yb_fixture(), log));
        let mut restore_on_exit = false;
        let success = finish_apply(&mut live, &mut restore_on_exit, Ok(Ok("applied".into())));
        assert_eq!(success.fan_result, Some(true));
        assert!(!success.busy);
        let failure = finish_apply(
            &mut live,
            &mut restore_on_exit,
            Ok(Err(ApplyError::Wmi(WmiError::NotAvailable("test failure")))),
        );
        assert_eq!(failure.fan_result, Some(false));
        assert!(failure.error);
        assert!(!failure.busy);
        drop(live);
        std::fs::remove_file(dir.join("aero-control.log")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}

#[cfg(test)]
mod gpu_session_tests {
    use super::*;

    fn completed(session: &FanSession) -> SessionEvent {
        loop {
            let event = session
                .rx
                .recv_timeout(Duration::from_secs(3))
                .expect("mock command completes");
            if !event.busy {
                return event;
            }
        }
    }

    fn with_disabled_gpu(action: impl FnOnce(&mut Live)) {
        let dir = std::env::temp_dir().join(format!(
            "aero-disabled-gpu-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut backend = Logged::new(
            MockAcpi::aero_17_yb_fixture(),
            CallLog::open_in(&dir).unwrap(),
        );
        backend.gpu_status = GpuStatus::Unavailable;
        backend
            .inner
            .throw_setters
            .insert("SetNvPowerConfig".into());
        let mut live = Live::Mock(backend);
        action(&mut live);
        let Live::Mock(backend) = &live else {
            unreachable!()
        };
        assert!(backend.inner.calls.iter().all(|call| !matches!(
            call.method.as_str(),
            "SetNvPowerConfig" | "GetNvPowerConfig" | "getGpuTemp1" | "getGpuTemp2"
        )));
        drop(live);
        std::fs::remove_file(dir.join("aero-control.log")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn unavailable_and_unknown_gpus_keep_cpu_and_fan_readings_without_gpu_queries() {
        for status in [
            GpuStatus::Unavailable,
            GpuStatus::Unknown("PnP failed".into()),
        ] {
            let mut reader = MockAcpi::aero_17_yb_fixture();
            let sensors = snapshot(&mut reader, &status);
            let power = power_snapshot(&mut reader, &status);
            assert_eq!(sensors.gpu_status, status);
            assert_eq!(sensors.gpu, "Unavailable");
            assert_eq!(sensors.gpu2, "Unavailable");
            assert!(sensors.error.is_none());
            assert!(sensors.cpu.ends_with("°C"));
            assert!(sensors.rpm1.contains("RPM"));
            assert!(sensors.rpm2.contains("RPM"));
            assert!(power.nv_power_config.is_err());
            assert!(power.nv_thermal_target.is_ok());
            assert!(power.fan_state.is_ok());
            assert!(reader.calls.iter().all(|call| !matches!(
                call.method.as_str(),
                "GetNvPowerConfig" | "getGpuTemp1" | "getGpuTemp2"
            )));
        }
    }

    #[test]
    fn all_fan_modes_curves_and_fixed_duty_work_without_nvidia_power_access() {
        with_disabled_gpu(|live| {
            for mode in [
                FanMode::Normal,
                FanMode::Quiet,
                FanMode::Gaming,
                FanMode::Fixed,
                FanMode::Max,
            ] {
                let outcome = Ok(apply_any(live, mode, 120, false));
                let event =
                    finish_fan_apply(live, &mut false, outcome, Some(mode == FanMode::Gaming));
                assert_eq!(event.fan_result, Some(true), "{:?}", event.message);
                assert_eq!(event.gpu_request, None);
                assert_eq!(
                    event
                        .sensors
                        .unwrap()
                        .power
                        .unwrap()
                        .fan_state
                        .unwrap()
                        .mode,
                    mode
                );
            }
            fixed_duty_any(live, 140, false).unwrap();
            let curve = aero_core::quiet_curve().points().to_vec();
            upload_any(live, curve.clone(), false).unwrap();
            upload_any(live, curve, true).unwrap();
            restore_any(live).unwrap();
        });
    }

    #[test]
    fn disabled_gpu_recovery_and_rejected_direct_requests_do_not_access_nvidia() {
        with_disabled_gpu(|live| {
            apply_any(live, FanMode::Quiet, 0, false).unwrap();
            let Live::Mock(backend) = live else {
                unreachable!()
            };
            let calls_before = backend.inner.calls.len();
            assert!(power_any(live, PowerWrite::NvPowerConfig(true)).is_err());
            let Live::Mock(backend) = live else {
                unreachable!()
            };
            assert_eq!(backend.inner.calls.len(), calls_before);
            assert_eq!(backend.inner.nv_thermal_target, 1);
            backend
                .inner
                .ignored_setters
                .insert("SetAutoFanStatus".into());
            let outcome = Ok(apply_any(live, FanMode::Gaming, 0, false));
            let event = finish_fan_apply(live, &mut false, outcome, Some(true));
            assert_eq!(event.fan_result, Some(false));
            assert_eq!(event.gpu_request, None);
            assert!(event.message.unwrap().contains("auto was restored"));
            assert_eq!(
                event
                    .sensors
                    .unwrap()
                    .power
                    .unwrap()
                    .fan_state
                    .unwrap()
                    .mode,
                FanMode::Normal
            );
        });
    }

    #[test]
    fn worker_profiles_restore_fans_without_gpu_requests_and_resume_requests_when_available() {
        let session = FanSession::start(BackendChoice::Mock);
        session.set_mock_gpu_status(GpuStatus::Unavailable);
        assert_eq!(
            completed(&session).sensors.unwrap().gpu_status,
            GpuStatus::Unavailable
        );
        let mut profile = aero_core::builtin_config()
            .profile("Gaming")
            .unwrap()
            .clone();
        profile.nv_power_config = Some(1);
        session.profile(profile.clone(), ApplyReason::Startup);
        let event = completed(&session);
        assert_eq!(event.fan_result, Some(true));
        assert_eq!(event.gpu_request, None);
        assert_eq!(
            event
                .sensors
                .unwrap()
                .power
                .unwrap()
                .fan_state
                .unwrap()
                .mode,
            FanMode::Gaming
        );
        session.power(PowerWrite::NvPowerConfig(true));
        let event = completed(&session);
        assert!(event.error);
        assert_eq!(event.fan_result, None);
        assert_eq!(
            event
                .sensors
                .unwrap()
                .power
                .unwrap()
                .fan_state
                .unwrap()
                .mode,
            FanMode::Gaming
        );
        session.set_mock_gpu_status(GpuStatus::Available);
        assert_eq!(
            completed(&session).sensors.unwrap().gpu_status,
            GpuStatus::Available
        );
        session.profile(profile, ApplyReason::Resume);
        let event = completed(&session);
        assert_eq!(event.fan_result, Some(true));
        assert_eq!(event.gpu_request, Some(Ok(true)));
    }

    #[test]
    fn max_test_shutdown_restores_firmware_auto_without_gpu_access() {
        with_disabled_gpu(|live| {
            apply_any(live, FanMode::Max, 229, false).unwrap();
            let (command_tx, command_rx) = mpsc::channel();
            let (event_tx, _event_rx) = mpsc::channel();
            command_tx.send(Command::Shutdown).unwrap();
            assert!(wait_or_shutdown(
                &command_rx,
                &event_tx,
                live,
                Duration::from_secs(10)
            ));
            restore_any(live).unwrap();
            let sensors = read_sensors_and_power(live).unwrap();
            assert_eq!(
                sensors.power.unwrap().fan_state.unwrap().mode,
                FanMode::Normal
            );
        });
    }
}

fn finish_fan_apply(
    live: &mut Live,
    restore_on_exit: &mut bool,
    outcome: Result<Result<String, ApplyError>, Box<dyn std::any::Any + Send>>,
    requested_gpu: Option<bool>,
) -> SessionEvent {
    let mut event = finish_apply(live, restore_on_exit, outcome);
    event.gpu_request = if !live.gpu_status().allows_commands() {
        None
    } else if event.error {
        Some(Err("The GPU request is unconfirmed after recovery.".into()))
    } else {
        requested_gpu.map(Ok)
    };
    event
}

fn finish_apply(
    live: &mut Live,
    restore_on_exit: &mut bool,
    outcome: Result<Result<String, ApplyError>, Box<dyn std::any::Any + Send>>,
) -> SessionEvent {
    match outcome {
        Ok(Ok(text)) => SessionEvent {
            sensors: safe_read_sensors_and_power(live),
            message: Some(text),
            error: false,
            busy: false,
            fan_result: Some(true),
            gpu_request: None,
            cpu_power: None,
        },
        Ok(Err(err)) => {
            // Invalid data can fail before the core guarded sequence starts.
            let err = match err {
                ApplyError::Restored { .. } | ApplyError::RestoreFailed { .. } => err,
                other => match safe_restore(live) {
                    Ok(()) => ApplyError::Restored {
                        reason: other.to_string(),
                    },
                    Err(restore) => ApplyError::RestoreFailed {
                        reason: other.to_string(),
                        restore,
                    },
                },
            };
            *restore_on_exit = matches!(err, ApplyError::RestoreFailed { .. });
            SessionEvent {
                sensors: safe_read_sensors_and_power(live),
                message: Some(err.to_string()),
                error: true,
                busy: false,
                fan_result: Some(false),
                gpu_request: None,
                cpu_power: None,
            }
        }
        Err(_) => {
            let restored = safe_restore(live);
            *restore_on_exit = restored.is_err();
            SessionEvent {
                sensors: safe_read_sensors_and_power(live),
                message: Some(restored.map_or_else(
                    |error| format!("Fan command panicked; recovery failed: {error}"),
                    |_| "The fan command panicked. Normal mode restored.".into(),
                )),
                error: true,
                busy: false,
                fan_result: Some(false),
                gpu_request: None,
                cpu_power: None,
            }
        }
    }
}

fn safe_read_sensors_and_power(live: &mut Live) -> Option<SensorSnapshot> {
    catch_unwind(AssertUnwindSafe(|| read_sensors_and_power(live)))
        .ok()
        .flatten()
}

fn report_from(label: &str, outcome: Result<String, ApplyError>) -> SessionEvent {
    match outcome {
        Ok(text) => SessionEvent {
            sensors: None,
            message: Some(format!("{label}: {text}")),
            error: false,
            busy: true,
            fan_result: None,
            gpu_request: None,
            cpu_power: None,
        },
        Err(err) => SessionEvent {
            sensors: None,
            message: Some(err.to_string()),
            error: true,
            busy: true,
            fan_result: None,
            gpu_request: None,
            cpu_power: None,
        },
    }
}

enum Live {
    Mock(Logged<MockAcpi>),
    #[cfg(windows)]
    Read(Logged<aero_wmi::WindowsReader>),
    #[cfg(windows)]
    Write(Logged<aero_wmi::WindowsWriter>),
}

impl Live {
    fn gpu_status(&self) -> &GpuStatus {
        match self {
            Self::Mock(backend) => &backend.gpu_status,
            #[cfg(windows)]
            Self::Read(backend) => &backend.gpu_status,
            #[cfg(windows)]
            Self::Write(backend) => &backend.gpu_status,
        }
    }

    fn gpu_power(&self) -> GpuPower {
        if self.gpu_status().allows_commands() {
            GpuPower::Available
        } else {
            GpuPower::Unavailable
        }
    }

    fn refresh_gpu_status(&mut self) {
        // Mock status is controlled by its fixture, never by the host machine.
        #[cfg(windows)]
        if !matches!(self, Self::Mock(_)) {
            let status = crate::gpu::detect(false);
            match self {
                Self::Read(backend) => backend.gpu_status = status,
                Self::Write(backend) => backend.gpu_status = status,
                Self::Mock(_) => unreachable!(),
            }
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Mock(_) => "MOCK",
            #[cfg(windows)]
            Self::Read(_) | Self::Write(_) => "REAL",
        }
    }

    fn writes(&self) -> bool {
        match self {
            Self::Mock(_) => true,
            #[cfg(windows)]
            Self::Read(_) => false,
            #[cfg(windows)]
            Self::Write(_) => true,
        }
    }

    fn banner(&self) -> String {
        match self {
            Self::Mock(_) => String::new(),
            #[cfg(windows)]
            Self::Read(_) => "Not running as Administrator. Temperatures and RPM are read-only. Close this window and start it with Run as administrator to change fans.".to_string(),
            #[cfg(windows)]
            Self::Write(_) => String::new(),
        }
    }
}

fn hello_line(live: &Live, log_path: &str) -> String {
    format!(
        "{}|{}|{log_path}|{}",
        live.label(),
        if live.writes() { "1" } else { "0" },
        live.banner()
    )
}

fn open_backend(choice: BackendChoice, log: Option<CallLog>) -> Result<Live, String> {
    let log = match log {
        Some(log) => log,
        None => {
            CallLog::open_in(std::env::temp_dir().join("aero-control-logs")).map_err(|err| err)?
        }
    };
    match choice {
        BackendChoice::Mock => {
            let mut mock = MockAcpi::aero_17_yb_fixture();
            mock.auto = 0;
            mock.consume_nv_power_requests = true;
            Ok(Live::Mock(Logged::new(mock, log)))
        }
        #[cfg(windows)]
        BackendChoice::Real { elevated: false } => {
            let reader = aero_wmi::WindowsReader::connect().map_err(|err| err.to_string())?;
            Ok(Live::Read(Logged::new(reader, log)))
        }
        #[cfg(windows)]
        BackendChoice::Real { elevated: true } => {
            let writer = aero_wmi::WindowsWriter::connect().map_err(|err| err.to_string())?;
            Ok(Live::Write(Logged::new(writer, log)))
        }
        #[cfg(not(windows))]
        BackendChoice::Real { .. } => Err("this build has no Windows WMI backend".to_string()),
    }
}

fn apply_any(live: &mut Live, mode: FanMode, raw: u8, expert: bool) -> Result<String, ApplyError> {
    let gpu = live.gpu_power();
    let report = match live {
        Live::Mock(backend) => apply_mode_with_gpu(backend, &mut NoPacer, mode, raw, expert, gpu)?,
        #[cfg(windows)]
        Live::Write(backend) => {
            apply_mode_with_gpu(backend, &mut FixedPacer::standard(), mode, raw, expert, gpu)?
        }
        #[cfg(windows)]
        Live::Read(_) => {
            return Err(ApplyError::Wmi(WmiError::NotAvailable(
                "fan writes need an elevated aero-gui",
            )))
        }
    };
    Ok(format!(
        "{mode:?} applied in {} attempt(s). {}",
        report.attempts,
        report.warnings.join(" ")
    ))
}

fn fixed_duty_any(live: &mut Live, raw: u8, expert: bool) -> Result<String, ApplyError> {
    let gpu = live.gpu_power();
    let report = match live {
        Live::Mock(backend) => apply_fixed_duty_with_gpu(backend, &mut NoPacer, raw, expert, gpu)?,
        #[cfg(windows)]
        Live::Write(backend) => {
            apply_fixed_duty_with_gpu(backend, &mut FixedPacer::standard(), raw, expert, gpu)?
        }
        #[cfg(windows)]
        Live::Read(_) => {
            return Err(ApplyError::Wmi(WmiError::NotAvailable(
                "fan writes need an elevated aero-gui",
            )))
        }
    };
    Ok(format!(
        "Fixed duty set to raw {} in {} attempt(s). {}",
        clamp_fixed_duty(raw, expert),
        report.attempts,
        report.warnings.join(" ")
    ))
}

fn upload_any(
    live: &mut Live,
    points: Vec<CurvePoint>,
    slots_only: bool,
) -> Result<String, ApplyError> {
    let curve = FanCurve::try_new(points).map_err(ApplyError::InvalidCurve)?;
    let gpu = live.gpu_power();
    let report = match (live, slots_only) {
        (Live::Mock(backend), false) => upload_curve_with_gpu(backend, &mut NoPacer, &curve, gpu)?,
        (Live::Mock(backend), true) => update_curve_with_gpu(backend, &mut NoPacer, &curve, gpu)?,
        #[cfg(windows)]
        (Live::Write(backend), false) => {
            upload_curve_with_gpu(backend, &mut FixedPacer::standard(), &curve, gpu)?
        }
        #[cfg(windows)]
        (Live::Write(backend), true) => {
            update_curve_with_gpu(backend, &mut FixedPacer::standard(), &curve, gpu)?
        }
        #[cfg(windows)]
        (Live::Read(_), _) => {
            return Err(ApplyError::Wmi(WmiError::NotAvailable(
                "curve upload needs an elevated aero-gui",
            )))
        }
    };
    Ok(format!(
        "Uploaded 15 slots in {} attempt(s). {}",
        report.attempts,
        report.warnings.join(" ")
    ))
}

fn power_any(live: &mut Live, write: PowerWrite) -> Result<String, ApplyError> {
    if matches!(write, PowerWrite::NvPowerConfig(_)) && !live.gpu_status().allows_commands() {
        return Err(ApplyError::Wmi(WmiError::NotAvailable(
            "NVIDIA power control is unavailable while the GPU is off or its status is unknown",
        )));
    }
    match live {
        Live::Mock(backend) => run_power(backend, &mut NoPacer, write),
        #[cfg(windows)]
        Live::Write(backend) => run_power(backend, &mut FixedPacer::standard(), write),
        #[cfg(windows)]
        Live::Read(_) => Err(ApplyError::Wmi(WmiError::NotAvailable(
            "battery and power writes need an elevated aero-gui",
        ))),
    }
}

fn safe_restore(live: &mut Live) -> Result<(), String> {
    match catch_unwind(AssertUnwindSafe(|| {
        apply_any(live, FanMode::Normal, 0, false)
    })) {
        Ok(result) => result.map(|_| ()).map_err(|error| error.to_string()),
        Err(_) => Err("Normal recovery panicked.".into()),
    }
}

fn run_power<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    write: PowerWrite,
) -> Result<String, ApplyError> {
    match write {
        PowerWrite::Charge(policy) => {
            set_charge_policy(writer, pacer, policy)?;
            Ok(match policy {
                ChargePolicy::Standard => "Charging policy set to Standard.".to_string(),
                ChargePolicy::Custom(stop) => format!("Charging stops at {stop}%."),
            })
        }
        PowerWrite::NvPowerConfig(on) => {
            set_nv_power_config(writer, pacer, on)?;
            Ok(format!(
                "GPU Level {} requested. Actual GPU power is not measured.",
                if on { 2 } else { 1 }
            ))
        }
        PowerWrite::ThermalTarget(quiet) => {
            set_nv_thermal_target(writer, pacer, quiet)?;
            Ok(format!(
                "GPU thermal profile set to {}.",
                if quiet { "Quiet" } else { "Standard" }
            ))
        }
        PowerWrite::Camera(on) => {
            set_flag_verified(writer, pacer, "SetCamera", "GetCamera", on)?;
            Ok(format!(
                "Camera {}.",
                if on { "enabled" } else { "disabled" }
            ))
        }
        PowerWrite::SleepUsbCharge(on) => {
            set_flag_verified(writer, pacer, "SetSleepUSBCharge", "GetSleepUSBCharge", on)?;
            Ok(format!(
                "USB charging during sleep {}.",
                if on { "enabled" } else { "disabled" }
            ))
        }
        PowerWrite::HibernationUsbCharge(on) => {
            set_flag_verified(
                writer,
                pacer,
                "SetHibernationUSBCharge",
                "GetHibernationUSBCharge",
                on,
            )?;
            Ok(format!(
                "USB charging during hibernation {}.",
                if on { "enabled" } else { "disabled" }
            ))
        }
        PowerWrite::DisplayOff => {
            let outcome = writer.set("SetBrightnessOff", &WriteParams::Data(1))?;
            if let Some(detail) = outcome.threw {
                return Err(WmiError::CallFailed {
                    method: "SetBrightnessOff".into(),
                    detail,
                }
                .into());
            }
            Ok("Display power-off request sent; panel state is not verified.".into())
        }
    }
}

fn set_flag_verified<W: AcpiWriter, P: Pacer>(
    writer: &mut W,
    pacer: &mut P,
    setter: &str,
    getter: &str,
    enabled: bool,
) -> Result<(), ApplyError> {
    let expected = u8::from(enabled);
    let outcome = writer.set(setter, &WriteParams::Data(expected))?;
    if let Some(detail) = outcome.threw {
        return Err(WmiError::CallFailed {
            method: setter.into(),
            detail,
        }
        .into());
    }
    pacer.pause();
    let output = writer.get(getter, &[])?;
    let actual = output.get("Data").and_then(|value| value.as_u32());
    if actual != Some(u32::from(expected)) {
        return Err(WmiError::CallFailed {
            method: getter.into(),
            detail: format!("requested {expected}, but readback returned {actual:?}"),
        }
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod firmware_control_tests {
    use super::*;

    #[test]
    fn device_changes_verify_only_the_requested_setting() {
        for (write, setter, getter, expected) in [
            (PowerWrite::Camera(false), "SetCamera", "GetCamera", 0),
            (
                PowerWrite::SleepUsbCharge(true),
                "SetSleepUSBCharge",
                "GetSleepUSBCharge",
                1,
            ),
            (
                PowerWrite::HibernationUsbCharge(true),
                "SetHibernationUSBCharge",
                "GetHibernationUSBCharge",
                1,
            ),
            (
                PowerWrite::ThermalTarget(true),
                "SetNvThermalTarget",
                "GetNvThermalTarget",
                1,
            ),
        ] {
            let mut writer = MockAcpi::aero_17_yb_fixture();
            let initial_fans = (
                writer.auto,
                writer.fixed,
                writer.step,
                writer.fixed_speed,
                writer.gpu_duty,
            );
            let mut pacer = aero_wmi::CountPacer::default();
            run_power(&mut writer, &mut pacer, write).unwrap();
            assert_eq!(writer.calls.len(), 2);
            assert_eq!(writer.calls[0].method, setter);
            assert_eq!(writer.calls[0].params, [("Data".to_string(), expected)]);
            assert_eq!(writer.calls[1].method, getter);
            assert_eq!(pacer.pauses, 1);
            assert_eq!(
                (
                    writer.auto,
                    writer.fixed,
                    writer.step,
                    writer.fixed_speed,
                    writer.gpu_duty
                ),
                initial_fans
            );
        }
    }

    #[test]
    fn silent_noop_and_transport_failure_do_not_report_success_or_retry() {
        let mut writer = MockAcpi::aero_17_yb_fixture();
        writer.ignored_setters.insert("SetCamera".into());
        let error = run_power(&mut writer, &mut NoPacer, PowerWrite::Camera(false)).unwrap_err();
        assert!(error.to_string().contains("readback"));
        assert_eq!(writer.calls_named("SetCamera").count(), 1);
        assert_eq!(writer.camera, 1);

        writer.calls.clear();
        writer.throw_setters.insert("SetCamera".into());
        assert!(run_power(&mut writer, &mut NoPacer, PowerWrite::Camera(false)).is_err());
        assert_eq!(writer.calls.len(), 1);
        assert_eq!(writer.calls[0].method, "SetCamera");
    }

    #[test]
    fn snapshots_expose_confirmed_device_state_and_model_without_writes() {
        let mut reader = MockAcpi::aero_17_yb_fixture();
        reader.camera = 0;
        reader.sleep_usb_charge = 1;
        reader.hibernation_usb_charge = 0;
        reader.nv_thermal_target = 1;
        let power = power_snapshot(&mut reader, &GpuStatus::Available);
        assert_eq!(power.camera, Ok(0));
        assert_eq!(power.sleep_usb_charge, Ok(1));
        assert_eq!(power.hibernation_usb_charge, Ok(0));
        assert_eq!(power.nv_thermal_target, Ok(1));
        assert_eq!(power.identity.unwrap().model.as_deref(), Some("AERO 17 YB"));
        assert!(reader
            .calls
            .iter()
            .all(|call| call.class == aero_wmi::GET_CLASS));
    }

    #[test]
    fn ordinary_sensor_polls_do_not_repeat_device_or_identity_reads() {
        let mut reader = MockAcpi::aero_17_yb_fixture();
        snapshot(&mut reader, &GpuStatus::Available);
        assert!(reader.calls.iter().all(|call| ![
            "GetCamera",
            "GetSleepUSBCharge",
            "GetHibernationUSBCharge",
            "GetNvThermalTarget",
        ]
        .contains(&call.method.as_str())));
    }

    #[test]
    fn display_request_matches_official_data_and_never_touches_the_mock_fans() {
        let mut writer = MockAcpi::aero_17_yb_fixture();
        let message = run_power(&mut writer, &mut NoPacer, PowerWrite::DisplayOff).unwrap();
        assert!(message.contains("not verified"));
        assert_eq!(writer.calls.len(), 1);
        assert_eq!(writer.calls[0].method, "SetBrightnessOff");
        assert_eq!(writer.calls[0].params, [("Data".to_string(), 1)]);
    }
}

fn restore_any(live: &mut Live) -> Result<(), ApplyError> {
    let gpu = live.gpu_power();
    match live {
        Live::Mock(backend) => restore_firmware_auto_with_gpu(backend, &mut NoPacer, gpu),
        #[cfg(windows)]
        Live::Write(backend) => {
            restore_firmware_auto_with_gpu(backend, &mut FixedPacer::standard(), gpu)
        }
        #[cfg(windows)]
        Live::Read(_) => Err(ApplyError::Wmi(WmiError::NotAvailable(
            "recovery requires elevation",
        ))),
    }
}

fn read_sensors(live: &mut Live) -> Option<SensorSnapshot> {
    let gpu_status = live.gpu_status().clone();
    match live {
        Live::Mock(backend) => Some(snapshot(backend, &gpu_status)),
        #[cfg(windows)]
        Live::Read(backend) => Some(snapshot(backend, &gpu_status)),
        #[cfg(windows)]
        Live::Write(backend) => Some(snapshot(backend, &gpu_status)),
    }
}

fn read_sensors_and_power(live: &mut Live) -> Option<SensorSnapshot> {
    let gpu_status = live.gpu_status().clone();
    let power = match live {
        Live::Mock(backend) => power_snapshot(backend, &gpu_status),
        #[cfg(windows)]
        Live::Read(backend) => power_snapshot(backend, &gpu_status),
        #[cfg(windows)]
        Live::Write(backend) => power_snapshot(backend, &gpu_status),
    };
    read_sensors(live).map(|sensors| SensorSnapshot {
        power: Some(power),
        ..sensors
    })
}

fn power_snapshot<R: AcpiReader + SkuSource>(
    reader: &mut R,
    gpu_status: &GpuStatus,
) -> PowerSnapshot {
    let mut read = |method: &str| {
        reader
            .get(method, &[])
            .map_err(|err| err.to_string())?
            .get("Data")
            .and_then(|value| value.as_u32())
            .ok_or_else(|| format!("{method} returned no Data"))
    };
    let charge_policy = read("GetChargePolicy");
    let charge_stop = read("GetChargeStop");
    let battery_health = read("GetBatteryHealth");
    let nv_power_config = if gpu_status.allows_commands() {
        read("GetNvPowerConfig")
    } else {
        Err("NVIDIA GPU is off or unavailable; its power flag was not read.".into())
    };
    let nv_thermal_target = read("GetNvThermalTarget");
    let camera = read("GetCamera");
    let sleep_usb_charge = read("GetSleepUSBCharge");
    let hibernation_usb_charge = read("GetHibernationUSBCharge");
    let identity = reader.read_sku().map_err(|error| error.to_string());
    let fan_state = crate::fan_state::read(reader, &nv_thermal_target);
    PowerSnapshot {
        charge_policy,
        charge_stop,
        battery_health,
        nv_power_config,
        nv_thermal_target,
        camera,
        sleep_usb_charge,
        hibernation_usb_charge,
        identity,
        fan_state,
    }
}

fn snapshot<R: AcpiReader>(reader: &mut R, gpu_status: &GpuStatus) -> SensorSnapshot {
    let mut errors = Vec::new();
    let cpu = read_temp(reader, "getCpuTemp", &mut errors);
    let (gpu, gpu2) = if gpu_status.allows_commands() {
        (
            read_temp(reader, "getGpuTemp1", &mut errors),
            read_temp(reader, "getGpuTemp2", &mut errors),
        )
    } else {
        ("Unavailable".into(), "Unavailable".into())
    };
    let rpm1 = read_rpm(reader, "getRpm1", &mut errors);
    let rpm2 = read_rpm(reader, "getRpm2", &mut errors);
    let flags = format!(
        "auto {} · fixed {} · step {} · adjust {}",
        read_flag(reader, "GetAutoFanStatus", &mut errors),
        read_flag(reader, "GetFixedFanStatus", &mut errors),
        read_flag(reader, "GetStepFanStatus", &mut errors),
        read_flag(reader, "GetFanAdjustStatus", &mut errors),
    );
    SensorSnapshot {
        gpu_status: gpu_status.clone(),
        cpu,
        gpu,
        gpu2,
        rpm1,
        rpm2,
        flags,
        error: if errors.is_empty() {
            None
        } else {
            Some(errors.join(" | "))
        },
        power: None,
    }
}

fn read_temp<R: AcpiReader>(reader: &mut R, method: &str, errors: &mut Vec<String>) -> String {
    match reader.get(method, &[]) {
        Ok(output) => output
            .get("Data")
            .and_then(|value| value.as_u32())
            .map(|temp| format!("{temp}°C"))
            .unwrap_or_else(|| "—".to_string()),
        Err(err) => {
            errors.push(format!("{method}: {err}"));
            "error".to_string()
        }
    }
}

fn read_rpm<R: AcpiReader>(reader: &mut R, method: &str, errors: &mut Vec<String>) -> String {
    match reader.get(method, &[]) {
        Ok(output) => match output.get("Data").and_then(|value| value.as_u32()) {
            Some(raw) => {
                let reading = decode_rpm(raw, RpmOrder::AutoPreferSwap);
                if reading.ambiguous {
                    format!("{} RPM?", reading.rpm)
                } else {
                    format!("{} RPM", reading.rpm)
                }
            }
            None => "—".to_string(),
        },
        Err(err) => {
            errors.push(format!("{method}: {err}"));
            "error".to_string()
        }
    }
}

fn read_flag<R: AcpiReader>(reader: &mut R, method: &str, errors: &mut Vec<String>) -> String {
    match reader.get(method, &[]) {
        Ok(output) => output
            .get("Data")
            .and_then(|value| value.as_u32())
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".to_string()),
        Err(WmiError::MethodNotFound { .. }) => "n/a".to_string(),
        Err(err) => {
            errors.push(format!("{method}: {err}"));
            "error".to_string()
        }
    }
}

struct Logged<T> {
    inner: T,
    log: CallLog,
    gpu_status: GpuStatus,
}

impl<T> Logged<T> {
    fn new(inner: T, log: CallLog) -> Self {
        Self {
            inner,
            log,
            // The real worker replaces this with a PnP reading before its
            // first snapshot; mocks keep the enabled-GPU fixture by default.
            gpu_status: GpuStatus::Available,
        }
    }
}

impl<T: SkuSource> SkuSource for Logged<T> {
    fn read_sku(&mut self) -> Result<SkuInfo, WmiError> {
        self.inner.read_sku()
    }
}

impl<T: AcpiReader> AcpiReader for Logged<T> {
    fn class_status(&mut self, class: &str) -> Result<ClassStatus, WmiError> {
        self.inner.class_status(class)
    }

    fn get(&mut self, method: &str, params: &[(&str, u8)]) -> Result<WmiOutput, WmiError> {
        let result = self.inner.get(method, params);
        let text = match &result {
            Ok(output) => format!(
                "OK {}",
                output
                    .properties
                    .iter()
                    .map(|(name, value)| format!("{name}={value:?}"))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Err(err) => format!("ERR {err}"),
        };
        self.log
            .record(&format_call("GET", method, &fmt_params(params), &text));
        result
    }
}

impl<T: AcpiWriter> AcpiWriter for Logged<T> {
    fn set(&mut self, method: &str, params: &WriteParams) -> Result<SetOutcome, WmiError> {
        let shown = params
            .pairs()
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(",");
        let result = self.inner.set(method, params);
        let text = match &result {
            Ok(outcome) => {
                let returned = fmt_bytes(&outcome.returned);
                match &outcome.threw {
                    Some(err) => format!("ERR-AFTER-APPLY {err} returned={returned}"),
                    None => format!("OK returned={returned}"),
                }
            }
            Err(err) => format!("ERR {err}"),
        };
        self.log.record(&format_call("SET", method, &shown, &text));
        result
    }
}

fn fmt_bytes(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        "-".to_string()
    } else {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn fmt_params(params: &[(&str, u8)]) -> String {
    if params.is_empty() {
        "-".to_string()
    } else {
        params
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(",")
    }
}
