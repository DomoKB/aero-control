//! Owns the embedded-controller connection on one thread.
//!
//! `WindowsWriter` is tied to the COM apartment that created it, so the UI
//! thread only sends commands. Sensor polls are getters. They never choose a
//! fan duty.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use aero_core::{
    apply_fixed_duty, apply_mode, clamp_fixed_duty, restore_firmware_auto, set_charge_policy,
    set_nv_power_config, upload_curve, ApplyError, ChargePolicy, CurvePoint, FanCurve, FanMode,
};
#[cfg(windows)]
use aero_wmi::FixedPacer;
use aero_wmi::{
    decode_rpm, AcpiReader, AcpiWriter, ClassStatus, MockAcpi, NoPacer, Pacer, RpmOrder,
    SetOutcome, WmiError, WmiOutput, WriteParams,
};

use crate::log_file::{format_call, CallLog};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendChoice {
    Mock,
    /// Real `GB_WMIACPI` connection. Writes require an elevated token.
    Real {
        elevated: bool,
    },
}

#[derive(Clone, Debug)]
pub struct SensorSnapshot {
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
}

#[derive(Clone, Copy, Debug)]
pub enum PowerWrite {
    Charge(ChargePolicy),
    NvPowerConfig(bool),
}

#[derive(Clone, Debug)]
pub struct SessionEvent {
    pub sensors: Option<SensorSnapshot>,
    pub message: Option<String>,
    pub error: bool,
    pub busy: bool,
}

enum Command {
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
    MaxTest,
    Power(PowerWrite),
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
    pub fn start(choice: BackendChoice) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let join = thread::spawn(move || worker(choice, cmd_rx, ev_tx));
        let hello = ev_rx.recv().unwrap_or_else(|_| SessionEvent {
            sensors: None,
            message: Some("the fan thread stopped before it connected".to_string()),
            error: true,
            busy: false,
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

    pub fn max_test(&self) {
        let _ = self.tx.send(Command::MaxTest);
    }

    pub fn power(&self, write: PowerWrite) {
        let _ = self.tx.send(Command::Power(write));
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
    let writes = parts.next().unwrap_or("1") == "1";
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
                message: Some(format!("MOCK|0|{log_path}|{err}")),
                error: true,
                busy: false,
            });
            return;
        }
    };
    let _ = tx.send(SessionEvent {
        sensors: read_sensors_and_power(&mut live),
        message: Some(hello_line(&live, &log_path)),
        error: false,
        busy: false,
    });
    let mut restore_on_exit = false;
    loop {
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                if restore_on_exit {
                    restore_any(&mut live);
                }
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = tx.send(SessionEvent {
                    sensors: read_sensors(&mut live),
                    message: None,
                    error: false,
                    busy: false,
                });
            }
            Ok(Command::Apply { mode, raw, expert }) => {
                let _ = tx.send(busy());
                let outcome =
                    catch_unwind(AssertUnwindSafe(|| apply_any(&mut live, mode, raw, expert)));
                restore_on_exit = matches!(mode, FanMode::Fixed | FanMode::Max);
                let _ = tx.send(finish_apply(&mut live, &mut restore_on_exit, outcome));
            }
            Ok(Command::FixedDuty { raw, expert }) => {
                let _ = tx.send(busy());
                let outcome =
                    catch_unwind(AssertUnwindSafe(|| fixed_duty_any(&mut live, raw, expert)));
                restore_on_exit = true;
                let _ = tx.send(finish_apply(&mut live, &mut restore_on_exit, outcome));
            }
            Ok(Command::Upload(points)) => {
                let _ = tx.send(busy());
                let outcome = catch_unwind(AssertUnwindSafe(|| upload_any(&mut live, points)));
                restore_on_exit = false;
                let _ = tx.send(finish_apply(&mut live, &mut restore_on_exit, outcome));
            }
            Ok(Command::Power(write)) => {
                let _ = tx.send(busy());
                let outcome = catch_unwind(AssertUnwindSafe(|| power_any(&mut live, write)));
                let (message, error) = match outcome {
                    Ok(Ok(text)) => (text, false),
                    Ok(Err(err)) => (err.to_string(), true),
                    Err(_) => ("The power command panicked.".to_string(), true),
                };
                let _ = tx.send(SessionEvent {
                    sensors: read_sensors_and_power(&mut live),
                    message: Some(message),
                    error,
                    busy: false,
                });
            }
            Ok(Command::MaxTest) => {
                let _ = tx.send(SessionEvent {
                    sensors: None,
                    message: Some("Max for 10 seconds, then firmware auto.".to_string()),
                    error: false,
                    busy: true,
                });
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    apply_any(&mut live, FanMode::Max, 229, false)
                }));
                if outcome.is_err() {
                    restore_any(&mut live);
                    restore_on_exit = false;
                    let _ = tx.send(SessionEvent {
                        sensors: read_sensors(&mut live),
                        message: Some("Max panicked; firmware auto was restored.".to_string()),
                        error: true,
                        busy: false,
                    });
                    continue;
                }
                let applied = outcome.unwrap_or_else(|_| {
                    Err(ApplyError::Wmi(WmiError::NotAvailable("Max panicked")))
                });
                let _ = tx.send(report_from("Max", applied));
                if wait_or_shutdown(&rx, &tx, &mut live, Duration::from_secs(10)) {
                    restore_any(&mut live);
                    break;
                }
                let restored = catch_unwind(AssertUnwindSafe(|| restore_any(&mut live)));
                restore_on_exit = false;
                let failed = restored.is_err();
                let _ = tx.send(SessionEvent {
                    sensors: read_sensors(&mut live),
                    message: Some(if failed {
                        "Restore after the Max test panicked.".to_string()
                    } else {
                        "Max test finished. Firmware auto restored.".to_string()
                    }),
                    error: failed,
                    busy: false,
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
                });
            }
        }
    }
    false
}

fn busy() -> SessionEvent {
    SessionEvent {
        sensors: None,
        message: Some("Sending the firmware sequence…".to_string()),
        error: false,
        busy: true,
    }
}

fn finish_apply(
    live: &mut Live,
    restore_on_exit: &mut bool,
    outcome: Result<Result<String, ApplyError>, Box<dyn std::any::Any + Send>>,
) -> SessionEvent {
    match outcome {
        Ok(Ok(text)) => SessionEvent {
            sensors: read_sensors(live),
            message: Some(text),
            error: false,
            busy: false,
        },
        Ok(Err(err)) => {
            *restore_on_exit = false;
            SessionEvent {
                sensors: read_sensors(live),
                message: Some(err.to_string()),
                error: true,
                busy: false,
            }
        }
        Err(_) => {
            restore_any(live);
            *restore_on_exit = false;
            SessionEvent {
                sensors: read_sensors(live),
                message: Some("The fan command panicked. Firmware auto was restored.".to_string()),
                error: true,
                busy: false,
            }
        }
    }
}

fn report_from(label: &str, outcome: Result<String, ApplyError>) -> SessionEvent {
    match outcome {
        Ok(text) => SessionEvent {
            sensors: None,
            message: Some(format!("{label}: {text}")),
            error: false,
            busy: true,
        },
        Err(err) => SessionEvent {
            sensors: None,
            message: Some(err.to_string()),
            error: true,
            busy: true,
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
        BackendChoice::Mock => Ok(Live::Mock(Logged::new(MockAcpi::aero_17_yb_fixture(), log))),
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
    let report = match live {
        Live::Mock(backend) => apply_mode(backend, &mut NoPacer, mode, raw, expert)?,
        #[cfg(windows)]
        Live::Write(backend) => {
            apply_mode(backend, &mut FixedPacer::standard(), mode, raw, expert)?
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
    let report = match live {
        Live::Mock(backend) => apply_fixed_duty(backend, &mut NoPacer, raw, expert)?,
        #[cfg(windows)]
        Live::Write(backend) => {
            apply_fixed_duty(backend, &mut FixedPacer::standard(), raw, expert)?
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

fn upload_any(live: &mut Live, points: Vec<CurvePoint>) -> Result<String, ApplyError> {
    let curve = FanCurve::try_new(points).map_err(ApplyError::InvalidCurve)?;
    let report = match live {
        Live::Mock(backend) => upload_curve(backend, &mut NoPacer, &curve)?,
        #[cfg(windows)]
        Live::Write(backend) => upload_curve(backend, &mut FixedPacer::standard(), &curve)?,
        #[cfg(windows)]
        Live::Read(_) => {
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
            Ok(format!("NvPowerConfig set to {}.", u8::from(on)))
        }
    }
}

fn restore_any(live: &mut Live) {
    match live {
        Live::Mock(backend) => {
            let _ = restore_firmware_auto(backend, &mut NoPacer);
        }
        #[cfg(windows)]
        Live::Write(backend) => {
            let _ = restore_firmware_auto(backend, &mut FixedPacer::standard());
        }
        #[cfg(windows)]
        Live::Read(_) => {}
    }
}

fn read_sensors(live: &mut Live) -> Option<SensorSnapshot> {
    match live {
        Live::Mock(backend) => Some(snapshot(backend)),
        #[cfg(windows)]
        Live::Read(backend) => Some(snapshot(backend)),
        #[cfg(windows)]
        Live::Write(backend) => Some(snapshot(backend)),
    }
}

fn read_sensors_and_power(live: &mut Live) -> Option<SensorSnapshot> {
    let power = match live {
        Live::Mock(backend) => power_snapshot(backend),
        #[cfg(windows)]
        Live::Read(backend) => power_snapshot(backend),
        #[cfg(windows)]
        Live::Write(backend) => power_snapshot(backend),
    };
    read_sensors(live).map(|sensors| SensorSnapshot {
        power: Some(power),
        ..sensors
    })
}

fn power_snapshot<R: AcpiReader>(reader: &mut R) -> PowerSnapshot {
    let mut read = |method: &str| {
        reader
            .get(method, &[])
            .map_err(|err| err.to_string())?
            .get("Data")
            .and_then(|value| value.as_u32())
            .ok_or_else(|| format!("{method} returned no Data"))
    };
    PowerSnapshot {
        charge_policy: read("GetChargePolicy"),
        charge_stop: read("GetChargeStop"),
        battery_health: read("GetBatteryHealth"),
        nv_power_config: read("GetNvPowerConfig"),
    }
}

fn snapshot<R: AcpiReader>(reader: &mut R) -> SensorSnapshot {
    let mut errors = Vec::new();
    let cpu = read_temp(reader, "getCpuTemp", &mut errors);
    let gpu = read_temp(reader, "getGpuTemp1", &mut errors);
    let gpu2 = read_temp(reader, "getGpuTemp2", &mut errors);
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
}

impl<T> Logged<T> {
    fn new(inner: T, log: CallLog) -> Self {
        Self { inner, log }
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
