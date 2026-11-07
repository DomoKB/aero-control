//! Status view and curve editor.
//!
//! On Windows the default backend is `WindowsWriter`. `--mock` and non-Windows
//! builds keep the fake controller. The firmware controls the fans.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use aero_core::{
    issues, parse_custom_fan_table_text, ChargePolicy, CurveIssue, CurvePoint, FanCurve, FanMode,
    FIXED_FLOOR_PERCENT,
};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Frame, Layout, Margin, Pos2, RichText, Rounding, Sense,
    Stroke, TextStyle, Vec2,
};

use crate::session::{BackendChoice, FanSession, PowerSnapshot, PowerWrite, SensorSnapshot};
use crate::system_power::{Battery, Plan, SystemPower};

mod apply_state;
mod cpu_power;
mod curve_chart;
mod display_controls;
mod firmware_controls;
mod gauge;
mod kbd_preview;
mod keyboard;
mod live_duty;
mod profiles;
mod startup;
mod system_controls;
use keyboard::KeyboardTab;
use live_duty::{Input, LiveDuty};

const CHARGE_POLICY_CUSTOM: u8 = 4;
const HISTORY_SECS: f64 = 60.0;

const BG: Color32 = Color32::from_rgb(14, 17, 22);
const BAR: Color32 = Color32::from_rgb(18, 22, 28);
const SIDE: Color32 = Color32::from_rgb(16, 20, 26);
const CARD: Color32 = Color32::from_rgb(26, 32, 42);
const CARD_RAISED: Color32 = Color32::from_rgb(34, 42, 54);
const STROKE: Color32 = Color32::from_rgb(52, 64, 80);
const TEXT: Color32 = Color32::from_rgb(232, 236, 242);
const MUTED: Color32 = Color32::from_rgb(165, 177, 192);
const ACCENT: Color32 = Color32::from_rgb(64, 196, 168);
const ON_ACCENT: Color32 = Color32::from_rgb(10, 24, 22);
const WARN: Color32 = Color32::from_rgb(214, 154, 72);
const DANGER: Color32 = Color32::from_rgb(214, 96, 88);
const AMBER: Color32 = Color32::from_rgb(232, 184, 74);
const HOT: Color32 = Color32::from_rgb(220, 72, 64);

pub struct AeroApp {
    session: FanSession,
    identity: aero_wmi::SkuInfo,
    elevated: bool,
    busy: bool,
    report_error: bool,
    sensor_error: Option<String>,
    arm_max_test: bool,
    page: Page,
    points: Vec<CurvePoint>,
    issues: Vec<CurveIssue>,
    drag_index: Option<usize>,
    selected_curve_point: usize,
    fan_apply: apply_state::ApplyState,
    fixed_percent: u8,
    live_duty: LiveDuty,
    expert: bool,
    arm_max: bool,
    import_text: String,
    import_note: String,
    report: String,
    sensors: Sensors,
    history: History,
    started: Instant,
    keyboard: KeyboardTab,
    firmware_power: Option<PowerSnapshot>,
    /// Last request sent in this session; not inferred from transient TEMQ.
    last_gpu_request: Option<bool>,
    cpu_power: cpu_power::CpuPowerTab,
    charge_custom: bool,
    charge_stop: u8,
    system: SystemPower,
    plans: Vec<Plan>,
    active_plan: Result<Plan, String>,
    battery: Option<Battery>,
    profiles: profiles::ProfilesTab,
    controls: system_controls::ControlsTab,
    display: display_controls::DisplayTab,
    desktop: crate::desktop_events::DesktopEvents,
    startup_checked: bool,
    startup_setup_checked: bool,
    last_power_source: Option<bool>,
    queued_profile: Option<QueuedApply>,
    pending_profile_name: Option<String>,
    last_resume: Option<Instant>,
    tray_requested: bool,
    exit_requested: bool,
    hotkey_requested: bool,
    wifi_hotkey_requested: bool,
    display_off_hotkey_requested: bool,
    startup_hide_pending: bool,
    hotkey_restore: Option<aero_core::Profile>,
    startup: startup::StartupTab,
    background_launch: bool,
    open_window_requested: bool,
    // Drop ownership only after the hardware session and desktop worker stop.
    instance: crate::instance::Instance,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BackgroundAction {
    Wait,
    Open,
    Exit,
}

enum QueuedApply {
    Profile(aero_core::Profile, aero_core::ApplyReason),
    // Resolve the latest preference only when the worker is free, so a resume
    // during a user command cannot later restore a stale fan mode.
    FanRestore(aero_core::ApplyReason),
}

const MODES: [(&str, FanMode, &str); 6] = [
    (
        "Normal",
        FanMode::Normal,
        "Normal: firmware automatic control.",
    ),
    (
        "Quiet",
        FanMode::Quiet,
        "Quiet: firmware control with the quiet GPU thermal target, as Control Center.",
    ),
    (
        "Gaming",
        FanMode::Gaming,
        "Gaming: firmware boost fan curve and a Level 2 GPU power request, as Control Center.",
    ),
    (
        "Fixed",
        FanMode::Fixed,
        "Fixed: one duty for both fans. Set the dial; it applies when you let go.",
    ),
    (
        "Max",
        FanMode::Max,
        "Max: both fans at raw 229 (100%). Click twice.",
    ),
    (
        "Custom",
        FanMode::Custom,
        "Custom: the uploaded curve. Opens the Curve editor.",
    ),
];

#[derive(Clone, Copy)]
enum Reading {
    Temp(Option<f32>),
    Rpm(Option<f32>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Fans,
    Curve,
    Power,
    Battery,
    Keyboard,
    Log,
    Profiles,
    SystemControls,
}

struct Sensors {
    cpu: String,
    gpu: String,
    gpu2: String,
    rpm1: String,
    rpm2: String,
    flags: String,
}

struct History {
    cpu: Trace,
    gpu: Trace,
    rpm1: Trace,
    rpm2: Trace,
}

struct Trace {
    samples: VecDeque<(f64, f32)>,
}

impl Trace {
    fn new() -> Self {
        Self {
            samples: VecDeque::new(),
        }
    }

    fn push(&mut self, t: f64, value: Option<f32>) {
        let Some(value) = value else {
            return;
        };
        self.samples.push_back((t, value));
        while self
            .samples
            .front()
            .is_some_and(|(old, _)| t - *old > HISTORY_SECS)
        {
            self.samples.pop_front();
        }
    }
}

impl History {
    fn new() -> Self {
        Self {
            cpu: Trace::new(),
            gpu: Trace::new(),
            rpm1: Trace::new(),
            rpm2: Trace::new(),
        }
    }
}

impl AeroApp {
    pub fn new(
        choice: BackendChoice,
        background: bool,
        instance: crate::instance::Instance,
    ) -> Self {
        let mock = matches!(choice, BackendChoice::Mock);
        let points = aero_core::quiet_curve().points().to_vec();
        let issues = issues(&points);
        let session = FanSession::start(choice);
        let report = session.banner.clone().unwrap_or_else(|| {
            if mock {
                "Simulation mode. Changes affect test data only.".into()
            } else {
                "Ready.".into()
            }
        });
        let initial = session.initial_sensors.clone();
        let identity = initial
            .as_ref()
            .and_then(|snapshot| snapshot.power.as_ref())
            .and_then(|power| power.identity.as_ref().ok())
            .cloned()
            .unwrap_or_else(|| crate::device_identity::read(mock));
        let system = SystemPower::new(mock);
        let profiles = profiles::ProfilesTab::new(mock);
        let startup_hide_pending = !background && profiles.settings().start_minimized;
        let mut display = display_controls::DisplayTab::new(mock);
        // Invalid saved files fall back to opt-in defaults in ProfilesTab.
        let _ = display.set_settings(profiles.settings().display.clone());
        let mut app = Self {
            session,
            identity,
            elevated: matches!(choice, BackendChoice::Real { elevated: true }),
            busy: false,
            report_error: false,
            sensor_error: None,
            arm_max_test: false,
            page: Page::Fans,
            points: points.clone(),
            issues,
            drag_index: None,
            selected_curve_point: 0,
            fan_apply: apply_state::ApplyState::default(),
            fixed_percent: 50,
            live_duty: LiveDuty::new(50),
            expert: false,
            arm_max: false,
            import_text: "45,35;60,45;0,0;85,80;90,100".to_string(),
            import_note: String::new(),
            report,
            sensors: Sensors {
                cpu: "—".into(),
                gpu: "—".into(),
                gpu2: "—".into(),
                rpm1: "—".into(),
                rpm2: "—".into(),
                flags: "—".into(),
            },
            history: History::new(),
            started: Instant::now(),
            keyboard: KeyboardTab::new(mock),
            firmware_power: None,
            last_gpu_request: None,
            cpu_power: cpu_power::CpuPowerTab::default(),
            charge_custom: false,
            charge_stop: 80,
            plans: system.plans(),
            active_plan: system.active(),
            battery: system.battery(),
            system,
            profiles,
            controls: system_controls::ControlsTab::new(mock),
            display,
            desktop: crate::desktop_events::DesktopEvents::new(mock),
            startup_checked: false,
            startup_setup_checked: false,
            last_power_source: None,
            queued_profile: None,
            pending_profile_name: None,
            last_resume: None,
            tray_requested: false,
            exit_requested: false,
            hotkey_requested: false,
            wifi_hotkey_requested: false,
            display_off_hotkey_requested: false,
            startup_hide_pending,
            hotkey_restore: None,
            startup: startup::StartupTab::new(mock),
            background_launch: background,
            open_window_requested: false,
            instance,
        };
        if let Some(snapshot) = initial {
            app.apply_snapshot(snapshot);
        }
        if crate::device_identity::has_yb_command_set(&app.identity) {
            if let Some(power) = &app.firmware_power {
                if let Ok(state) = &power.fan_state {
                    app.fan_apply = apply_state::ApplyState::from_readback(state);
                    if let Some(raw) = state.fixed_raw {
                        app.fixed_percent = aero_core::raw_to_percent(raw);
                        app.expert = raw < aero_core::percent_to_raw(FIXED_FLOOR_PERCENT);
                        app.live_duty.applied(app.fixed_percent);
                    }
                    if let Some(note) = state.note {
                        app.report = format!("{} {note}", app.report);
                    }
                }
            }
        }
        app
    }

    pub fn attach_window(&mut self, ctx: &egui::Context) {
        install_theme(ctx);
    }

    /// No native control window or renderer exists during this loop. The same
    /// session and automation state are moved into the GUI when it is opened.
    pub fn poll_background(&mut self, ctx: &egui::Context) -> BackgroundAction {
        let _ = ctx.run(Default::default(), |ctx| {
            self.poll_session();
            self.poll_automation(ctx);
        });
        if self.exit_requested {
            BackgroundAction::Exit
        } else if self.open_window_requested
            || self.report_error
            || self.profiles.load_error().is_some()
            || !self.session.writes_enabled
            || self.desktop.last_error().is_some()
            || (!self.desktop.can_hide_to_tray()
                && self.started.elapsed() > Duration::from_secs(10))
        {
            // Fail visibly instead of leaving an inaccessible process or a
            // silently failed restoration in the notification area.
            BackgroundAction::Open
        } else {
            BackgroundAction::Wait
        }
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        self.open_window_requested = true;
        self.startup_hide_pending = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn poll_session(&mut self) {
        while let Some(event) = self.session.poll() {
            if let Some(request) = &event.gpu_request {
                self.last_gpu_request = request.as_ref().ok().copied();
            }
            if let Some(reading) = event.cpu_power {
                self.cpu_power.observe(reading);
            }
            let fresh_power = event
                .sensors
                .as_ref()
                .is_some_and(|snapshot| snapshot.power.is_some());
            self.busy = event.busy;
            if let Some(snapshot) = event.sensors {
                self.apply_snapshot(snapshot);
            }
            let remembered_fan = self.fan_apply.complete(event.fan_result);
            if fresh_power && event.fan_result != Some(false) {
                self.reconcile_fan_readback();
            }
            if let Some(succeeded) = event.fan_result {
                let name = self.pending_profile_name.take();
                if succeeded {
                    if let Some(profile) = remembered_fan {
                        if let Err(error) = self.profiles.remember_fan(profile, name.as_deref()) {
                            self.report = format!(
                                "Fan settings applied, but remembering them failed: {error}"
                            );
                            self.report_error = true;
                            continue;
                        }
                    }
                }
            }
            if let Some(message) = event.message {
                self.report = message;
                self.report_error = event.error;
            }
        }
    }

    fn apply_snapshot(&mut self, snapshot: SensorSnapshot) {
        let t = self.started.elapsed().as_secs_f64();
        self.sensors.cpu = snapshot.cpu;
        self.sensors.gpu = snapshot.gpu;
        self.sensors.gpu2 = snapshot.gpu2;
        self.sensors.rpm1 = snapshot.rpm1;
        self.sensors.rpm2 = snapshot.rpm2;
        self.sensors.flags = snapshot.flags;
        self.sensor_error = snapshot.error;
        if let Some(power) = snapshot.power {
            if let Ok(identity) = &power.identity {
                self.identity = identity.clone();
            }
            if let Ok(policy) = power.charge_policy {
                self.charge_custom = policy == u32::from(CHARGE_POLICY_CUSTOM);
            }
            if let Ok(stop) = power.charge_stop {
                self.charge_stop = stop.clamp(60, 100) as u8;
            }
            self.firmware_power = Some(power);
        }
        self.battery = self.system.battery();
        self.active_plan = self.system.active();
        self.history.cpu.push(t, parse_temp(&self.sensors.cpu));
        self.history.gpu.push(t, parse_temp(&self.sensors.gpu));
        self.history.rpm1.push(t, parse_rpm(&self.sensors.rpm1));
        self.history.rpm2.push(t, parse_rpm(&self.sensors.rpm2));
        // Startup mode detection uses the full readback; ordinary flag strings
        // alone cannot distinguish every fan mode.
    }

    fn recompute(&mut self) {
        self.issues = issues(&self.points);
    }

    fn reconcile_fan_readback(&mut self) {
        if self.busy || !crate::device_identity::has_yb_command_set(&self.identity) {
            return;
        }
        let Some(power) = &self.firmware_power else {
            return;
        };
        self.fan_apply.reconcile_readback(&power.fan_state);
        if let Some(raw) = self.fan_apply.applied_fixed_raw {
            if !self.live_duty.pending(self.fixed_percent) {
                self.fixed_percent = aero_core::raw_to_percent(raw);
                self.expert = raw < aero_core::percent_to_raw(FIXED_FLOOR_PERCENT);
                self.live_duty.applied(self.fixed_percent);
            }
        }
    }

    fn writes_allowed(&self) -> bool {
        self.session.writes_enabled
            && crate::device_identity::has_yb_command_set(&self.identity)
            && !self.busy
            && self.fan_apply.pending_mode.is_none()
    }

    fn apply_firmware(&mut self, mode: FanMode) {
        if !self.writes_allowed() {
            self.report = if self.busy {
                "A firmware command is already running.".to_string()
            } else if !crate::device_identity::has_yb_command_set(&self.identity) {
                "Fan commands are not verified for the detected laptop model.".to_string()
            } else {
                "Fan writes are off. Run aero-gui as Administrator.".to_string()
            };
            self.report_error = true;
            return;
        }
        let raw = aero_core::percent_to_raw(self.fixed_percent);
        self.queued_profile = None;
        self.hotkey_restore = None;
        if mode == FanMode::Fixed {
            self.live_duty.applied(self.fixed_percent);
        }
        self.busy = true;
        self.fan_apply.pending_mode = Some(mode);
        self.fan_apply.pending_fixed_raw = match mode {
            FanMode::Fixed => Some(aero_core::clamp_fixed_duty(raw, self.expert)),
            FanMode::Max => Some(229),
            _ => None,
        };
        self.session.apply(mode, raw, self.expert);
        self.arm_max = false;
        self.arm_max_test = false;
    }

    fn fixed_shown(&self) -> bool {
        self.fan_apply.pending_mode.or(self.fan_apply.active_mode) == Some(FanMode::Fixed)
    }

    fn push_live_duty(&mut self, ctx: &egui::Context) {
        if !self.fixed_shown() {
            return;
        }
        match self.live_duty.wait(self.fixed_percent, Instant::now()) {
            Some(wait) if wait.is_zero() && self.writes_allowed() => self.send_fixed_duty(),
            Some(wait) => ctx.request_repaint_after(wait.max(Duration::from_millis(50))),
            None => {}
        }
    }

    fn send_fixed_duty(&mut self) {
        if self.fan_apply.active_mode != Some(FanMode::Fixed)
            || self.fan_apply.pending_mode.is_some()
        {
            self.apply_firmware(FanMode::Fixed);
            return;
        }
        self.live_duty.applied(self.fixed_percent);
        self.busy = true;
        self.fan_apply.pending_mode = Some(FanMode::Fixed);
        self.fan_apply.pending_fixed_raw = Some(aero_core::clamp_fixed_duty(
            aero_core::percent_to_raw(self.fixed_percent),
            self.expert,
        ));
        self.session
            .fixed_duty(aero_core::percent_to_raw(self.fixed_percent), self.expert);
    }

    fn upload(&mut self) {
        if !self.writes_allowed() {
            self.report = "Curve upload needs an elevated aero-gui.".to_string();
            self.report_error = true;
            return;
        }
        match FanCurve::try_new(self.points.clone()) {
            Ok(curve) => {
                self.queued_profile = None;
                self.hotkey_restore = None;
                self.busy = true;
                self.fan_apply.pending_mode = Some(FanMode::Custom);
                let points = curve.points().to_vec();
                self.fan_apply.pending_curve = Some(points.clone());
                self.session.upload(points);
            }
            Err(issues) => {
                self.issues = issues;
                self.report = "Upload stayed disabled: the curve is not valid.".to_string();
                self.report_error = true;
            }
        }
    }

    fn push_curve(&mut self) {
        if self.fan_apply.active_mode == Some(FanMode::Custom)
            && self.fan_apply.pending_mode.is_none()
            && self.drag_index.is_none()
            && self.issues.is_empty()
            && self.writes_allowed()
            && self.points != self.fan_apply.applied_curve
        {
            self.fan_apply.pending_curve = Some(self.points.clone());
            self.busy = true;
            self.fan_apply.pending_mode = Some(FanMode::Custom);
            self.session.update_curve(self.points.clone());
        }
    }

    fn load_curve(&mut self, curve: aero_core::FanCurve, report: &str) {
        self.points = curve.points().to_vec();
        self.drag_index = None;
        self.selected_curve_point = 0;
        self.recompute();
        self.report = report.into();
        self.report_error = false;
    }

    fn apply_saved_profile(&mut self, profile: aero_core::Profile, reason: aero_core::ApplyReason) {
        if !self.writes_allowed() {
            return;
        }
        self.hotkey_restore = None;
        let curve = if profile.fan_mode == FanMode::Custom {
            match profile.fan_curve() {
                Ok(curve) => Some(curve.points().to_vec()),
                Err(_) => {
                    self.report = format!("{} contains an invalid curve.", profile.name);
                    self.report_error = true;
                    return;
                }
            }
        } else {
            None
        };
        self.busy = true;
        self.fan_apply.pending_mode = Some(profile.fan_mode);
        self.fan_apply.pending_fixed_raw = match profile.fan_mode {
            FanMode::Fixed => Some(aero_core::clamp_fixed_duty(
                profile.fixed_duty_raw.unwrap_or_else(|| {
                    aero_core::percent_to_raw(profile.fixed_duty_percent.unwrap_or(50))
                }),
                profile.expert_mode,
            )),
            FanMode::Max => Some(229),
            _ => None,
        };
        self.fan_apply.pending_curve = curve.clone();
        if let Some(curve) = curve {
            self.points = curve;
            self.recompute();
        }
        if profile.fan_mode == FanMode::Fixed {
            self.fixed_percent = profile
                .fixed_duty_raw
                .map(aero_core::raw_to_percent)
                .or(profile.fixed_duty_percent)
                .unwrap_or(50);
            self.live_duty.applied(self.fixed_percent);
        }
        self.pending_profile_name = Some(profile.name.clone());
        self.session.profile(profile, reason);
    }

    fn capture_profile(&self, name: String) -> Option<aero_core::Profile> {
        let mut profile = self.fan_apply.fan_profile(name)?;
        profile.cpu_power_preset = self.cpu_power.confirmed_preset();
        profile.expert_mode =
            self.expert || self.fan_apply.applied_fixed_raw.is_some_and(|raw| raw < 80);
        if let Some(power) = &self.firmware_power {
            profile.nv_power_config = self.last_gpu_request.map(u8::from);
            profile.nv_thermal_target = power
                .nv_thermal_target
                .as_ref()
                .ok()
                .and_then(|value| u8::try_from(*value).ok())
                .filter(|value| *value <= 1);
            profile.charge_policy = power
                .charge_policy
                .as_ref()
                .ok()
                .and_then(|value| u8::try_from(*value).ok())
                .filter(|value| *value == 0 || *value == 4);
            if power.charge_policy == Ok(4) {
                profile.charge_limit = power
                    .charge_stop
                    .as_ref()
                    .ok()
                    .and_then(|value| u8::try_from(*value).ok())
                    .filter(|value| (60..=100).contains(value));
            }
        }
        Some(profile)
    }

    fn profiles_ui(&mut self, ui: &mut egui::Ui) {
        let can_capture = self.writes_allowed()
            && self.firmware_power.is_some()
            && self.fan_apply.active_mode.is_some()
            && (self.fan_apply.active_mode != Some(FanMode::Fixed)
                || !self.live_duty.pending(self.fixed_percent));
        let can_apply = self.writes_allowed();
        let actions = ui
            .add_enabled_ui(!self.busy, |ui| {
                self.profiles.ui(ui, can_apply, can_capture)
            })
            .inner;
        for action in actions {
            match action {
                profiles::ProfileAction::Apply(profile) => {
                    self.queued_profile = None;
                    self.apply_saved_profile(profile, aero_core::ApplyReason::User);
                }
                profiles::ProfileAction::Capture(name) => {
                    if let Some(profile) = self.capture_profile(name) {
                        if let Err(error) = self.profiles.capture(profile) {
                            self.report = error;
                            self.report_error = true;
                        }
                    }
                }
                profiles::ProfileAction::AutomationChanged => {
                    self.queued_profile = None;
                    if let Err(error) = self
                        .display
                        .set_settings(self.profiles.settings().display.clone())
                    {
                        self.report = error;
                        self.report_error = true;
                    }
                }
            }
        }
        if let Some(error) = self.desktop.last_error() {
            ui.add_space(8.0);
            banner(ui, &error, WARN);
        }
        ui.add_space(12.0);
        if self.background_launch {
            ui.label("Started with Windows: this session stays in the notification area when closed. Choose Exit from its tray menu to stop it.");
        }
        self.startup.ui(ui);
    }

    fn setup_startup_once(&mut self, release_build: bool) {
        if !self.startup_setup_checked {
            self.startup_setup_checked = true;
            if release_build
                && self.elevated
                && self.session.writes_enabled
                && crate::device_identity::has_yb_command_set(&self.identity)
                && self.profiles.load_error().is_none()
                && !self.profiles.settings().startup_setup_complete
            {
                self.startup.setup_first_launch();
            }
        }
    }

    fn poll_automation(&mut self, ctx: &egui::Context) {
        self.setup_startup_once(!cfg!(debug_assertions));
        if let Some(result) = self.startup.poll() {
            let saved = result.and_then(|remember_choice| {
                if remember_choice {
                    self.profiles.mark_startup_setup_complete()
                } else {
                    Ok(())
                }
            });
            if let Err(error) = saved {
                self.report = format!("Windows startup: {error}");
                self.report_error = true;
            }
        }
        if self.instance.take_show_request() {
            self.show_window(ctx);
        }
        if let Some((message, error)) = self.controls.poll_background() {
            self.report = message;
            self.report_error = error;
        }
        let tray_requested = self.background_launch || self.profiles.settings().keep_in_tray;
        if tray_requested != self.tray_requested {
            self.desktop.set_tray_enabled(tray_requested);
            self.tray_requested = tray_requested;
        }
        let hotkey_requested = self.profiles.settings().fan_hotkey;
        if hotkey_requested != self.hotkey_requested {
            self.desktop.set_fan_hotkey_enabled(hotkey_requested);
            self.hotkey_requested = hotkey_requested;
        }
        let wifi_hotkey_requested = self.profiles.settings().wifi_hotkey;
        if wifi_hotkey_requested != self.wifi_hotkey_requested {
            self.desktop.set_wifi_hotkey_enabled(wifi_hotkey_requested);
            self.wifi_hotkey_requested = wifi_hotkey_requested;
        }
        let display_off_requested = self.profiles.settings().display_off_hotkey;
        if display_off_requested != self.display_off_hotkey_requested {
            self.desktop
                .set_display_off_hotkey_enabled(display_off_requested);
            self.display_off_hotkey_requested = display_off_requested;
        }
        let mut resume = false;
        while let Some(event) = self.desktop.poll() {
            match event {
                crate::desktop_events::DesktopEvent::Resume => resume = true,
                crate::desktop_events::DesktopEvent::PowerChanged => {
                    self.battery = self.system.battery()
                }
                crate::desktop_events::DesktopEvent::Show => {
                    self.show_window(ctx);
                }
                crate::desktop_events::DesktopEvent::FanMode(mode) => self.apply_firmware(mode),
                crate::desktop_events::DesktopEvent::FanHotkey => {
                    if self.hotkey_requested
                        && self.desktop.fan_hotkey_enabled()
                        && self.writes_allowed()
                    {
                        if let Some(profile) = self.hotkey_restore.take() {
                            self.apply_saved_profile(profile, aero_core::ApplyReason::User);
                            self.pending_profile_name = None;
                        } else {
                            let mut profile = self
                                .capture_profile("Previous fan mode".into())
                                .unwrap_or_else(|| {
                                    aero_core::builtin_config()
                                        .profile("Normal")
                                        .unwrap()
                                        .clone()
                                });
                            profile.charge_policy = None;
                            profile.charge_limit = None;
                            self.apply_firmware(FanMode::Max);
                            self.hotkey_restore = Some(profile);
                        }
                    }
                }
                crate::desktop_events::DesktopEvent::WifiHotkey => {
                    if self.wifi_hotkey_requested && self.desktop.wifi_hotkey_enabled() {
                        match self.controls.request_wifi_toggle() {
                            Ok(()) => {
                                self.report = "Requesting Wi-Fi change…".into();
                                self.report_error = false;
                            }
                            Err(error) => {
                                self.report = error;
                                self.report_error = true;
                            }
                        }
                    }
                }
                crate::desktop_events::DesktopEvent::DisplayOffHotkey => {
                    if self.display_off_hotkey_requested
                        && self.desktop.display_off_hotkey_enabled()
                        && self.writes_allowed()
                    {
                        self.send_power(PowerWrite::DisplayOff);
                    }
                }
                crate::desktop_events::DesktopEvent::Exit => {
                    self.exit_requested = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if self.startup_hide_pending {
            if self.desktop.can_hide_to_tray() {
                self.startup_hide_pending = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else if !tray_requested || self.desktop.last_error().is_some() {
                self.startup_hide_pending = false;
            }
        }
        if resume {
            self.battery = self.system.battery();
        }
        let on_ac = self.battery.and_then(|battery| battery.on_ac);
        if let Some((message, error)) = self.display.on_power_source_changed(on_ac) {
            self.report = message;
            self.report_error = error;
        }
        if !self.startup_checked {
            self.startup_checked = true;
            self.last_power_source = on_ac;
            if let Some(profile) = self.profiles.startup_profile(on_ac) {
                self.queued_profile = Some(QueuedApply::Profile(
                    profile,
                    aero_core::ApplyReason::Startup,
                ));
            } else {
                self.queued_profile =
                    Some(QueuedApply::FanRestore(aero_core::ApplyReason::Startup));
            }
        } else if let Some(on_ac) = on_ac {
            if self
                .last_power_source
                .is_some_and(|previous| previous != on_ac)
            {
                if let Some(profile) = self.profiles.automation_profile(on_ac) {
                    self.queued_profile = Some(QueuedApply::Profile(
                        profile,
                        aero_core::ApplyReason::AcDcChanged,
                    ));
                }
            }
            self.last_power_source = Some(on_ac);
        }
        if resume
            && self
                .last_resume
                .map_or(true, |last| last.elapsed() > Duration::from_secs(5))
        {
            self.last_resume = Some(Instant::now());
            self.battery = self.system.battery();
            if let Some(profile) = self
                .profiles
                .resume_profile(self.battery.and_then(|battery| battery.on_ac))
            {
                self.queued_profile = Some(QueuedApply::Profile(
                    profile,
                    aero_core::ApplyReason::Resume,
                ));
            } else if !matches!(self.queued_profile, Some(QueuedApply::Profile(_, _))) {
                self.queued_profile = Some(QueuedApply::FanRestore(aero_core::ApplyReason::Resume));
            }
        }
        if self.writes_allowed() {
            if let Some(request) = self.queued_profile.take() {
                match request {
                    QueuedApply::Profile(profile, reason) => {
                        self.apply_saved_profile(profile, reason)
                    }
                    QueuedApply::FanRestore(reason) => {
                        if let Some(profile) = self.profiles.fan_restore_profile() {
                            self.apply_saved_profile(profile, reason);
                            self.pending_profile_name = None;
                        }
                    }
                }
            }
        }
    }
}

impl eframe::App for AeroApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_session();
        self.poll_automation(ctx);
        if !self.exit_requested
            && self.desktop.can_hide_to_tray()
            && ctx.input(|input| input.viewport().close_requested())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        self.push_live_duty(ctx);
        self.push_curve();
        ctx.request_repaint_after(std::time::Duration::from_millis(400));

        egui::TopBottomPanel::top("bar")
            .frame(
                Frame::none()
                    .fill(BAR)
                    .inner_margin(Margin::symmetric(16.0, 12.0)),
            )
            .show(ctx, |ui| self.top_bar(ui));

        egui::TopBottomPanel::bottom("status")
            .frame(
                Frame::none()
                    .fill(BAR)
                    .inner_margin(Margin::symmetric(16.0, 8.0)),
            )
            .show(ctx, |ui| self.status_line(ui));

        egui::SidePanel::left("nav")
            .exact_width(196.0)
            .resizable(false)
            .frame(
                Frame::none()
                    .fill(SIDE)
                    .inner_margin(Margin::symmetric(12.0, 16.0)),
            )
            .show(ctx, |ui| self.nav(ui));

        egui::CentralPanel::default()
            .frame(Frame::none().fill(BG).inner_margin(Margin::same(20.0)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(("page", self.page as u8))
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.page {
                        Page::Fans => self.fans_ui(ui),
                        Page::Curve => self.curve_ui(ui),
                        Page::Power => self.power_ui(ui),
                        Page::Battery => self.battery_ui(ui),
                        Page::Keyboard => {
                            self.keyboard.ui(ui);
                            if let Some((text, error)) = self.keyboard.new_status() {
                                self.report = text;
                                self.report_error = error;
                            }
                        }
                        Page::Log => self.log_ui(ui),
                        Page::Profiles => self.profiles_ui(ui),
                        Page::SystemControls => {
                            if let Some((message, error)) = self.controls.ui(ui) {
                                self.report = message;
                                self.report_error = error;
                            }
                            ui.add_space(12.0);
                            let actions = firmware_controls::ui(
                                ui,
                                self.firmware_power.as_ref(),
                                self.writes_allowed(),
                                !self.busy,
                            );
                            if actions.refresh && !self.busy {
                                self.busy = true;
                                self.session.refresh_power();
                            }
                            if let Some(write) = actions.write {
                                self.send_power(write);
                            }
                            ui.add_space(12.0);
                            if let Some((message, error)) = self.display.ui(ui) {
                                self.report = message;
                                self.report_error = error;
                            }
                            if self.display.take_settings_changed() {
                                let next = self.display.settings().clone();
                                if let Err(error) = self.profiles.save_display_settings(next) {
                                    let _ = self
                                        .display
                                        .set_settings(self.profiles.settings().display.clone());
                                    self.report = format!(
                                        "Display switching settings were not saved: {error}"
                                    );
                                    self.report_error = true;
                                } else {
                                    self.display.settings_saved();
                                }
                            }
                        }
                    });
            });
    }
}

impl AeroApp {
    fn top_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("aero-control")
                    .size(20.0)
                    .strong()
                    .color(TEXT),
            );
            ui.label(
                RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                    .size(12.0)
                    .color(MUTED),
            );
            ui.label(
                RichText::new(crate::device_identity::model_name(&self.identity))
                    .size(13.0)
                    .color(MUTED),
            )
            .on_hover_text(crate::device_identity::support_note(&self.identity));
            if !crate::device_identity::has_yb_command_set(&self.identity) {
                ui.colored_label(WARN, "Model not verified");
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.session.backend_label != "REAL" {
                    pill(ui, "Simulation", Color32::from_rgb(96, 150, 214), ON_ACCENT);
                } else if !self.elevated {
                    pill(ui, "Administrator access needed", WARN, ON_ACCENT);
                }
            });
        });
    }

    fn status_line(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.busy {
                ui.spinner();
            }
            let color = if self.report_error { DANGER } else { MUTED };
            ui.colored_label(color, self.report.as_str());
        });
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        let previous_page = self.page;
        ui.label(RichText::new("SECTIONS").size(11.0).color(MUTED));
        ui.add_space(8.0);
        for (label, page) in [
            ("Fans", Page::Fans),
            ("Curve", Page::Curve),
            ("Power", Page::Power),
            ("Battery", Page::Battery),
            ("Keyboard", Page::Keyboard),
            ("Profiles", Page::Profiles),
            ("System", Page::SystemControls),
            ("Log", Page::Log),
        ] {
            nav_item(ui, label, self.page == page, &mut self.page, page);
            ui.add_space(4.0);
        }
        if self.page == Page::SystemControls && previous_page != self.page {
            self.controls.refresh_all();
            self.display.refresh();
        }
        if self.page == Page::Power && previous_page != self.page {
            self.cpu_power.enter();
        }
    }

    fn fans_ui(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "Fans", "Live readings. The firmware controls the fans.");
        if let Some(error) = &self.sensor_error {
            banner(ui, error, DANGER);
            ui.add_space(12.0);
        }
        if !crate::device_identity::has_yb_command_set(&self.identity) {
            banner(
                ui,
                &crate::device_identity::support_note(&self.identity),
                WARN,
            );
            ui.add_space(12.0);
        }
        if ui.available_width() > 960.0 {
            self.gauge_row(ui);
            ui.add_space(16.0);
            self.mode_card(ui);
        } else {
            self.mode_card(ui);
            ui.add_space(16.0);
            self.gauge_row(ui);
        }
        ui.add_space(10.0);
        ui.label(RichText::new(&self.sensors.flags).size(13.0).color(MUTED));
        ui.add_space(16.0);
    }

    fn gauge_row(&self, ui: &mut egui::Ui) {
        let gap = 16.0;
        let card_h = 252.0;
        let width = ui.available_width();
        let cols: usize = if width > 960.0 {
            4
        } else if width > 480.0 {
            2
        } else {
            1
        };
        let col_w = (width - gap * (cols as f32 - 1.0)) / cols as f32;
        let gpu2 = parse_temp(&self.sensors.gpu2)
            .filter(|c| *c > 0.0)
            .map(|c| format!("sensor 2  {c:.0}°C"));
        let rpm_note = |raw: &str| raw.ends_with('?').then(|| "byte order guessed".to_string());
        let cards = [
            (
                "CPU",
                Reading::Temp(parse_temp(&self.sensors.cpu)),
                &self.history.cpu,
                None,
            ),
            (
                "GPU",
                Reading::Temp(parse_temp(&self.sensors.gpu)),
                &self.history.gpu,
                gpu2,
            ),
            (
                "CPU fan",
                Reading::Rpm(parse_rpm(&self.sensors.rpm1)),
                &self.history.rpm1,
                rpm_note(&self.sensors.rpm1),
            ),
            (
                "GPU fan",
                Reading::Rpm(parse_rpm(&self.sensors.rpm2)),
                &self.history.rpm2,
                rpm_note(&self.sensors.rpm2),
            ),
        ];
        let rows = cards.len().div_ceil(cols);
        let origin = ui.cursor().min;
        for (index, (title, reading, trace, note)) in cards.into_iter().enumerate() {
            let rect = egui::Rect::from_min_size(
                origin
                    + Vec2::new(
                        (index % cols) as f32 * (col_w + gap),
                        (index / cols) as f32 * (card_h + gap),
                    ),
                Vec2::new(col_w, card_h),
            );
            paint_gauge_card(
                ui.painter(),
                rect,
                title,
                reading,
                &trace.samples,
                note.as_deref(),
            );
        }
        ui.add_space(rows as f32 * card_h + rows.saturating_sub(1) as f32 * gap);
    }

    fn mode_card(&mut self, ui: &mut egui::Ui) {
        let allow = self.writes_allowed();
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                section_title(ui, "Fan mode");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    self.max_test_button(ui, allow);
                    if ui
                        .add_enabled(allow, secondary_button("Restore auto"))
                        .on_hover_text("Applies the Control Center Normal fan and GPU settings.")
                        .clicked()
                    {
                        self.apply_firmware(FanMode::Normal);
                    }
                });
            });
            ui.add_space(10.0);
            self.mode_segments(ui, allow);
            ui.add_space(10.0);
            if self.fixed_shown() {
                self.fixed_controls(ui);
                return;
            }
            let hint = MODES
                .iter()
                .find(|(_, mode, _)| self.fan_apply.active_mode == Some(*mode))
                .map_or(
                    "Choose a mode to confirm and apply its settings.",
                    |(_, _, tip)| tip,
                );
            ui.label(RichText::new(hint).size(13.0).color(MUTED));
        });
    }

    fn mode_segments(&mut self, ui: &mut egui::Ui, allow: bool) {
        let items = MODES.map(|(label, mode, tip)| {
            let label = if mode == FanMode::Max && self.arm_max {
                "Confirm"
            } else {
                label
            };
            (label, tip, allow || mode == FanMode::Custom)
        });
        let on = MODES
            .iter()
            .position(|(_, mode, _)| self.fan_apply.active_mode == Some(*mode));
        let armed = MODES
            .iter()
            .position(|(_, mode, _)| *mode == FanMode::Max)
            .filter(|_| self.arm_max);
        let Some(index) = segmented(ui, "fan_mode", &items, on, armed) else {
            return;
        };
        match MODES[index].1 {
            FanMode::Custom => self.page = Page::Curve,
            FanMode::Max if !self.arm_max => {
                self.arm_max = true;
                self.report = "Max sets both fans to raw 229 (100%). Click Confirm.".into();
                self.report_error = false;
            }
            mode => self.apply_firmware(mode),
        }
    }

    fn max_test_button(&mut self, ui: &mut egui::Ui, allow: bool) {
        let label = if self.arm_max_test {
            "Confirm: Max for 10 s"
        } else {
            "Test: Max for 10 s"
        };
        if ui
            .add_enabled(allow, secondary_button(label).stroke(Stroke::new(1.0_f32, WARN)))
            .on_hover_text(
                "Sets both fans to raw 229 for 10 seconds, then restores firmware auto. Click twice.",
            )
            .clicked()
        {
            if self.arm_max_test {
                self.busy = true;
                self.fan_apply.pending_mode = Some(FanMode::Normal);
                self.fan_apply.pending_temporary = true;
                self.session.max_test();
                self.arm_max_test = false;
                self.arm_max = false;
            } else {
                self.arm_max_test = true;
                self.report =
                    "This sets both fans to raw 229 for 10 seconds, then restores firmware auto. Click the confirm button."
                        .to_string();
                self.report_error = false;
            }
        }
    }

    fn fixed_controls(&mut self, ui: &mut egui::Ui) {
        let floor = if self.expert { 0 } else { FIXED_FLOOR_PERCENT };
        self.fixed_percent = self.fixed_percent.max(floor);
        ui.horizontal(|ui| {
            let knob = gauge::knob(ui, &mut self.fixed_percent, floor, 150.0);
            let now = Instant::now();
            if knob.dragged() {
                self.live_duty.input(Input::Dragging, now);
            } else if knob.drag_stopped() || knob.clicked() {
                self.live_duty.input(Input::Released, now);
            } else if knob.changed() {
                self.live_duty.input(Input::Scrolled, now);
            }
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.add_space(22.0);
                ui.label(
                    RichText::new(format!("Both fans at {}%", self.fixed_percent))
                        .size(18.0)
                        .strong()
                        .color(TEXT),
                );
                ui.add_space(4.0);
                let (status, color) = if self.report_error && !self.busy {
                    ("Not applied, see the log", DANGER)
                } else if self.live_duty.held(self.fixed_percent) {
                    ("Release to apply", AMBER)
                } else if self.busy || self.live_duty.pending(self.fixed_percent) {
                    ("Applying…", AMBER)
                } else {
                    ("Live on both fans", ACCENT)
                };
                ui.horizontal(|ui| {
                    let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                    ui.painter().circle_filled(dot.center(), 4.0, color);
                    ui.label(RichText::new(status).size(13.0).color(color));
                });
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!(
                        "Drag or scroll the dial. It applies when you let go. Floor {FIXED_FLOOR_PERCENT}% unless expert is on."
                    ))
                    .size(13.0)
                    .color(MUTED),
                );
                ui.add_space(8.0);
                ui.checkbox(&mut self.expert, "Expert floor override")
                    .on_hover_text("Allows a fixed duty below 35% (raw 80).");
            });
        });
    }

    fn curve_ui(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Curve",
            "Edit the curve used by firmware to control both fans.",
        );
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                section_title(ui, "Custom curve");
                ui.add_space(8.0);
                self.curve_state(ui);
            });
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui.add(secondary_button("Load Quiet")).clicked() {
                    self.load_curve(aero_core::quiet_curve(), "Editor loaded the quiet curve.");
                }
                if ui.add(secondary_button("Load Factory")).clicked() {
                    self.load_curve(
                        aero_core::factory_curve(),
                        "Editor loaded the YB factory curve.",
                    );
                }
                if self.fan_apply.active_mode != Some(FanMode::Custom)
                    && ui
                        .add_enabled(
                            self.issues.is_empty() && self.writes_allowed(),
                            primary_button("Apply firmware curve"),
                        )
                        .on_hover_text("Applies the displayed curve to both fans.")
                        .clicked()
                {
                    self.upload();
                }
            });
            ui.add_space(6.0);
            let instruction = if self.fan_apply.active_mode == Some(FanMode::Custom) {
                if self.session.writes_enabled {
                    "Custom mode is active. Valid edits and loaded presets apply automatically."
                } else {
                    "Read-only: edits and loaded presets are not applied."
                }
            } else {
                "Preview only. Apply firmware curve uploads it to the controller."
            };
            ui.label(RichText::new(instruction).size(14.0).color(TEXT));
            ui.add_space(8.0);
            let markers: Vec<_> = [("CPU", &self.sensors.cpu), ("GPU", &self.sensors.gpu)]
                .into_iter()
                .filter_map(|(label, text)| {
                    parse_temp(text).map(|temp_c| curve_chart::Marker {
                        label,
                        temp_c,
                        color: gauge::temp_band(temp_c),
                    })
                })
                .collect();
            let height = (ui.available_height() - 110.0).clamp(240.0, 430.0);
            curve_chart::show(ui, &mut self.points, &mut self.drag_index, &markers, height);
            self.recompute();
            ui.add_space(6.0);
            self.selected_curve_point = self
                .selected_curve_point
                .min(self.points.len().saturating_sub(1));
            let suggestion = suggested_curve_point(&self.points);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        suggestion.is_some() && self.points.len() < 15,
                        secondary_button("Add point"),
                    )
                    .on_hover_text("Adds a point in the largest available gap; drag it to adjust.")
                    .clicked()
                {
                    if let Some((temp, percent)) = suggestion {
                        if aero_core::insert_point(&mut self.points, temp, percent).is_ok() {
                            self.selected_curve_point = self
                                .points
                                .iter()
                                .position(|point| point.temp_c == temp)
                                .unwrap_or(0);
                        }
                    }
                }
                ui.label(RichText::new("Point to remove:").color(TEXT));
                let selected = self
                    .points
                    .get(self.selected_curve_point)
                    .map(|point| {
                        format!(
                            "{} °C · {}%",
                            point.temp_c,
                            aero_core::raw_to_percent(point.duty_raw)
                        )
                    })
                    .unwrap_or_else(|| "No point".to_string());
                egui::ComboBox::from_id_salt("curve_point")
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for (index, point) in self.points.iter().enumerate() {
                            ui.selectable_value(
                                &mut self.selected_curve_point,
                                index,
                                format!(
                                    "{} °C · {}%",
                                    point.temp_c,
                                    aero_core::raw_to_percent(point.duty_raw)
                                ),
                            );
                        }
                    });
                if ui
                    .add_enabled(self.points.len() > 2, secondary_button("Remove point"))
                    .on_hover_text("Removes the point selected in the list.")
                    .clicked()
                {
                    let _ = aero_core::remove_point(&mut self.points, self.selected_curve_point);
                    self.selected_curve_point = self
                        .selected_curve_point
                        .min(self.points.len().saturating_sub(1));
                }
            });
            ui.label(
                RichText::new(
                    "Drag points to move them. Double-click to add; right-click to remove.",
                )
                .size(14.0)
                .color(TEXT),
            );
            self.recompute();
            for issue in &self.issues {
                let where_ = issue
                    .index
                    .map(|index| format!("Point {}: ", index + 1))
                    .unwrap_or_default();
                ui.colored_label(DANGER, format!("{where_}{}", issue.message));
            }
        });
        ui.add_space(12.0);
        egui::CollapsingHeader::new("Import CustomFanTable")
            .default_open(false)
            .show(ui, |ui| {
                ui.label("Paste from HKLM\\SOFTWARE\\Gigabyte\\SmartManager. This never writes that key.");
                ui.text_edit_singleline(&mut self.import_text);
                if ui.button("Parse pasted table").clicked() {
                    let parsed = parse_custom_fan_table_text(&self.import_text);
                    self.import_note = format!(
                        "{} point(s), {} dropped after a terminator. {}",
                        parsed.points.len(),
                        parsed.dropped_after_terminator.len(),
                        parsed.warnings.join(" ")
                    );
                    if parsed.points.len() >= 2 {
                        self.points = parsed.points;
                        self.recompute();
                    }
                }
                #[cfg(windows)]
                if ui.button("Read CustomFanTable from the registry").clicked() {
                    match aero_core::read_gcc_custom_fan_table() {
                        Ok(parsed) => {
                            self.import_note = format!(
                                "Registry {} — {} point(s). {}",
                                parsed.encoding,
                                parsed.points.len(),
                                parsed.warnings.join(" ")
                            );
                            if parsed.points.len() >= 2 {
                                self.points = parsed.points;
                                self.recompute();
                            }
                        }
                        Err(err) => self.import_note = err,
                    }
                }
                #[cfg(not(windows))]
                ui.label("Registry import is compiled in on Windows only.");
                if !self.import_note.is_empty() {
                    ui.label(&self.import_note);
                }
            });
    }

    fn curve_state(&self, ui: &mut egui::Ui) {
        let (text, fill) = if self.busy && self.fan_apply.pending_mode == Some(FanMode::Custom) {
            ("Applying…", AMBER)
        } else if self.fan_apply.active_mode == Some(FanMode::Custom) {
            if !self.session.writes_enabled {
                ("Active · read-only", WARN)
            } else if self.issues.is_empty() {
                ("Active · edits apply on release", ACCENT)
            } else {
                ("Active · fix the curve to apply", WARN)
            }
        } else if self.fan_apply.active_mode.is_none() {
            ("Preview · mode unknown", WARN)
        } else {
            ("Preview · not active", WARN)
        };
        let fg = if fill == CARD_RAISED {
            MUTED
        } else {
            ON_ACCENT
        };
        pill(ui, text, fill, fg);
    }

    fn power_ui(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Power",
            "CPU power limits, GPU performance and the Windows power plan.",
        );
        let allow_cpu = self.writes_allowed();
        if let Some(preset) = self.cpu_power.ui(ui, allow_cpu, self.busy, &self.identity) {
            self.busy = true;
            self.report = if preset.is_some() {
                "Applying and verifying CPU power limits…"
            } else {
                "Reading CPU limits…"
            }
            .into();
            self.report_error = false;
            self.session.cpu_power(preset);
        }
        ui.add_space(16.0);
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, "Windows power plan");
            ui.add_space(10.0);
            if self.plans.is_empty() {
                ui.label(
                    RichText::new("None of the standard plans exist on this system.").color(MUTED),
                );
            } else {
                let items: Vec<(&str, &str, bool)> = self
                    .plans
                    .iter()
                    .map(|plan| (plan.name, "", true))
                    .collect();
                let on = self
                    .plans
                    .iter()
                    .position(|plan| self.active_plan.as_ref() == Ok(plan));
                if let Some(index) = segmented(ui, "plan", &items, on, None) {
                    let plan = self.plans[index];
                    match self.system.set_active(plan) {
                        Ok(()) => {
                            self.report = format!("Windows power plan set to {}.", plan.name);
                            self.report_error = false;
                        }
                        Err(err) => {
                            self.report = err;
                            self.report_error = true;
                        }
                    }
                    self.active_plan = self.system.active();
                }
            }
            ui.add_space(8.0);
            let note = match &self.active_plan {
                Ok(plan) => format!(
                    "Active: {}. Switching does not need Administrator.",
                    plan.name
                ),
                Err(other) => format!("Active: {other}."),
            };
            ui.label(RichText::new(note).size(13.0).color(MUTED));
        });
        ui.add_space(16.0);
        let allow = self.writes_allowed();
        let current = self
            .firmware_power
            .as_ref()
            .map(|power| power.nv_power_config.clone());
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, "GPU performance");
            ui.add_space(10.0);
            let on = self.last_gpu_request.map(usize::from);
            let items = [("Level 1 · Standard", "", allow), ("Level 2 · Maximum", "", allow)];
            if let Some(index) = segmented(ui, "nv_power", &items, on, None) {
                self.send_power(PowerWrite::NvPowerConfig(index == 1));
            }
            ui.add_space(8.0);
            let reading = match current {
                Some(Ok(value)) => format!("Firmware request flag: {value}. This flag can reset after a request."),
                Some(Err(err)) => format!("GPU power read failed: {err}"),
                None => "GPU power configuration has not been read.".to_string(),
            };
            ui.label(RichText::new(reading).size(13.0).color(MUTED));
            ui.label(RichText::new(match self.last_gpu_request {
                Some(on) => format!("Last requested in this session: Level {}.", if on { 2 } else { 1 }),
                None => "No GPU level requested in this session. The current power budget is unknown.".into(),
            }).color(MUTED));
            ui.label(
                RichText::new(
                    "Uses Control Center’s two GPU requests for this model. Selection shows the last request, not measured GPU power. Fan modes can also send a GPU request; other apps or firmware may change the power budget.",
                )
                .size(13.0)
                .color(MUTED),
            );
        });
        ui.add_space(16.0);
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, "GPU thermal profile");
            ui.label("Choose the standard target or the quieter target used by Control Center.");
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(allow, secondary_button("Apply Standard")).clicked() {
                    self.send_power(PowerWrite::ThermalTarget(false));
                }
                if ui.add_enabled(allow, secondary_button("Apply Quiet")).clicked() {
                    self.send_power(PowerWrite::ThermalTarget(true));
                }
            });
            let reading = match self.firmware_power.as_ref().map(|power| &power.nv_thermal_target) {
                Some(Ok(0)) => "Current: Standard.".to_string(),
                Some(Ok(1)) => "Current: Quiet.".to_string(),
                Some(Ok(value)) => format!("Unknown thermal profile value: {value}."),
                Some(Err(error)) => format!("Thermal profile read failed: {error}"),
                None => "Thermal profile has not been read.".to_string(),
            };
            ui.label(RichText::new(reading).color(MUTED));
            ui.label(RichText::new("Firmware readback confirms the selected profile; it does not measure resulting GPU temperature or power.").color(MUTED));
        });
        ui.add_space(16.0);
        info_card(
            ui,
            "Model support",
            &format!("{} CPU presets require a supported processor and a compatible Intel XTU installation. AI and Pantone integrations are excluded. MUX and Dynamic Boost controls are not implemented.", crate::device_identity::support_note(&self.identity)),
        );
    }

    fn battery_ui(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Battery",
            "Charging policy is kept by the embedded controller.",
        );
        let power = self.firmware_power.clone();
        let battery = self.battery;
        ui.columns(2, |cols| {
            card().show(&mut cols[0], |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(118.0);
                section_title(ui, "Windows battery");
                ui.add_space(8.0);
                battery_status(ui, battery);
            });
            card().show(&mut cols[1], |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(118.0);
                section_title(ui, "Battery health");
                ui.add_space(8.0);
                let code = match power.as_ref().map(|power| &power.battery_health) {
                    Some(Ok(code)) => code.to_string(),
                    Some(Err(_)) => "error".to_string(),
                    None => "—".to_string(),
                };
                ui.label(RichText::new(code).size(34.0).color(TEXT));
                let health = match power
                    .as_ref()
                    .and_then(|power| power.battery_health.as_ref().ok())
                {
                    Some(1) => "Good · Control Center health status.",
                    Some(2) => "Aging · Control Center health status.",
                    _ => "Unknown health status. The raw firmware code is shown above.",
                };
                ui.label(RichText::new(health).size(13.0).color(MUTED));
            });
        });
        ui.add_space(16.0);
        let allow = self.writes_allowed();
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, "Charging policy");
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                for (label, limit) in [
                    ("Mostly plugged in · 60%", 60),
                    ("Balanced · 80%", 80),
                    ("Full charge · 100%", 100),
                ] {
                    if ui.button(label).clicked() {
                        self.charge_custom = limit != 100;
                        self.charge_stop = limit;
                    }
                }
            });
            ui.label(RichText::new("Choose a preset, then Apply to change charging.").color(MUTED));
            let items = [
                ("Standard", "Charge to 100%.", true),
                ("Custom", "Stop charging at the percent below.", true),
            ];
            if let Some(index) = segmented(
                ui,
                "charge",
                &items,
                Some(usize::from(self.charge_custom)),
                None,
            ) {
                self.charge_custom = index == 1;
            }
            ui.add_space(12.0);
            ui.add_enabled_ui(self.charge_custom, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Stop at").color(TEXT));
                    ui.spacing_mut().slider_width = (ui.available_width() - 120.0).max(160.0);
                    ui.add(
                        egui::Slider::new(&mut self.charge_stop, 60..=100)
                            .suffix("%")
                            .trailing_fill(true),
                    );
                });
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let policy = if self.charge_custom {
                    ChargePolicy::Custom(self.charge_stop)
                } else {
                    ChargePolicy::Standard
                };
                if ui
                    .add_enabled(allow, primary_button("Apply"))
                    .on_hover_text(
                        "Custom sends SetChargePolicy 4, then SetChargeStop. Both are read back.",
                    )
                    .clicked()
                {
                    self.send_power(PowerWrite::Charge(policy));
                }
                ui.label(
                    RichText::new(firmware_charge_text(power.as_ref()))
                        .size(13.0)
                        .color(MUTED),
                );
            });
        });
    }

    fn send_power(&mut self, write: PowerWrite) {
        if !self.writes_allowed() {
            self.report = if !crate::device_identity::has_yb_command_set(&self.identity) {
                "Firmware writes are not verified for the detected laptop model.".to_string()
            } else {
                "Battery and power writes need an elevated aero-gui and an idle session."
                    .to_string()
            };
            self.report_error = true;
            return;
        }
        self.busy = true;
        self.session.power(write);
    }

    fn log_ui(&self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Log").size(22.0).strong().color(TEXT));
        ui.add_space(8.0);
        info_card(ui, "WMI log", &self.session.log_path);
        ui.add_space(10.0);
        info_card(ui, "Last result", &self.report);
        if let Some(error) = &self.sensor_error {
            ui.add_space(10.0);
            banner(ui, error, DANGER);
        }
        if let Some(banner_text) = &self.session.banner {
            ui.add_space(10.0);
            banner(ui, banner_text, WARN);
        }
        ui.add_space(12.0);
        egui::CollapsingHeader::new("About")
            .default_open(false)
            .show(ui, |ui| {
                ui.label("The firmware controls the fans. Sensor readings are for display. Thermal protection still belongs to the CPU, GPU and embedded controller.");
                ui.add_space(6.0);
                ui.label("Curve upload writes all 15 slots, reads them back, and restores firmware auto on a mismatch or a panic. Closing the window does the same if the last command was Fixed or Max.");
                ui.add_space(6.0);
                ui.label("The editor starts from a reference quiet curve. Factory loads the reference Control Center curve, including its exact raw duty values. Their descriptions identify the model where they were verified.");
            });
    }
}

fn install_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = Color32::from_rgb(22, 27, 34);
    visuals.extreme_bg_color = Color32::from_rgb(10, 13, 18);
    visuals.faint_bg_color = CARD;
    visuals.code_bg_color = CARD;
    visuals.hyperlink_color = ACCENT;
    visuals.warn_fg_color = WARN;
    visuals.error_fg_color = DANGER;
    visuals.window_rounding = Rounding::same(12.0);
    visuals.menu_rounding = Rounding::same(10.0);
    visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(64, 196, 168, 48);
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    let rounding = Rounding::same(8.0);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.rounding = rounding;
        widget.fg_stroke.color = TEXT;
    }
    visuals.widgets.noninteractive.fg_stroke.color = MUTED;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.widgets.active.bg_fill = ACCENT.linear_multiply(0.25);
    visuals.widgets.inactive.bg_fill = CARD_RAISED;
    ctx.set_visuals(visuals);

    ctx.all_styles_mut(|style| {
        style
            .text_styles
            .insert(TextStyle::Small, FontId::proportional(13.0));
        style
            .text_styles
            .insert(TextStyle::Body, FontId::proportional(16.0));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::proportional(15.0));
        style
            .text_styles
            .insert(TextStyle::Heading, FontId::proportional(26.0));
        style
            .text_styles
            .insert(TextStyle::Monospace, FontId::monospace(14.0));
        style.spacing.item_spacing = Vec2::new(12.0, 8.0);
        style.spacing.button_padding = Vec2::new(14.0, 8.0);
        style.spacing.interact_size.y = 32.0;
        style.spacing.window_margin = Margin::same(12.0);
    });
}

fn pill(ui: &mut egui::Ui, text: &str, fill: Color32, fg: Color32) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(13.0), fg);
    let pad = Vec2::new(12.0, 5.0);
    let size = galley.size() + pad * 2.0;
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, Rounding::same(size.y / 2.0), fill);
    let pos = Pos2::new(
        rect.center().x - galley.size().x / 2.0,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(pos, galley, fg);
}

fn nav_item(ui: &mut egui::Ui, label: &str, on: bool, page: &mut Page, value: Page) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 40.0), Sense::click());
    if response.clicked() {
        *page = value;
    }
    if on {
        ui.painter().rect_filled(
            rect,
            Rounding::same(8.0),
            Color32::from_rgba_unmultiplied(64, 196, 168, 36),
        );
        let bar = egui::Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height()));
        ui.painter().rect_filled(bar, Rounding::ZERO, ACCENT);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, Rounding::same(8.0), CARD);
    }
    ui.painter().text(
        rect.left_center() + Vec2::new(16.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(15.0),
        if on { TEXT } else { MUTED },
    );
}

/// Pick a point inside the widest available gap without changing neighbouring duties.
fn suggested_curve_point(points: &[CurvePoint]) -> Option<(u8, u8)> {
    let first = points.first()?;
    // Include both edge intervals. Keep suggestions within the editor's lower
    // limit and the core's maximum final temperature, so insertion stays valid.
    let mut boundaries = vec![29];
    boundaries.extend(points.iter().filter_map(|point| {
        (30..=aero_core::LAST_TEMP_MAX)
            .contains(&point.temp_c)
            .then_some(point.temp_c)
    }));
    boundaries.push(aero_core::LAST_TEMP_MAX + 1);
    boundaries
        .windows(2)
        .filter_map(|pair| {
            let low = pair[0] + 1;
            let high = pair[1] - 1;
            (low <= high).then(|| {
                let temp = low + (high - low) / 2;
                let duty = aero_core::duty_at(points, temp).unwrap_or(first.duty_raw);
                (high - low, temp, aero_core::raw_to_percent(duty))
            })
        })
        .max_by_key(|entry| entry.0)
        .map(|(_, temp, percent)| (temp, percent))
}

#[cfg(test)]
mod curve_editor_tests {
    use super::*;

    #[test]
    fn add_point_can_use_space_before_the_first_point() {
        let curve = aero_core::curve_from_percent(&[(85, 80), (86, 100)]);
        let mut points = curve.points().to_vec();
        let (temp, percent) = suggested_curve_point(&points).unwrap();
        assert!((30..85).contains(&temp));
        aero_core::insert_point(&mut points, temp, percent).unwrap();
        assert!(issues(&points).is_empty());
    }

    #[test]
    fn add_point_can_use_space_after_the_last_point() {
        let curve = aero_core::curve_from_percent(&[(30, 80), (31, 100)]);
        let mut points = curve.points().to_vec();
        let (temp, percent) = suggested_curve_point(&points).unwrap();
        assert!((32..=aero_core::LAST_TEMP_MAX).contains(&temp));
        aero_core::insert_point(&mut points, temp, percent).unwrap();
        assert!(issues(&points).is_empty());
    }

    #[test]
    fn add_point_does_not_select_an_existing_temperature() {
        let points = aero_core::quiet_curve().points().to_vec();
        let (temp, _) = suggested_curve_point(&points).unwrap();
        assert!(!points.iter().any(|point| point.temp_c == temp));
        assert!(suggested_curve_point(&[]).is_none());
    }
}

fn page_header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).size(24.0).strong().color(TEXT));
    ui.label(RichText::new(subtitle).color(MUTED));
    ui.add_space(16.0);
}

fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(17.0).strong().color(TEXT));
}

fn card() -> Frame {
    Frame::none()
        .fill(CARD)
        .rounding(Rounding::same(16.0))
        .stroke(Stroke::new(1.0_f32, STROKE))
        .inner_margin(Margin::same(18.0))
}

/// Equal-width segmented buttons. Returns the clicked index.
fn segmented(
    ui: &mut egui::Ui,
    id: &str,
    items: &[(&str, &str, bool)],
    on: Option<usize>,
    armed: Option<usize>,
) -> Option<usize> {
    let height = 46.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    ui.painter()
        .rect(rect, Rounding::same(12.0), BG, Stroke::new(1.0_f32, STROKE));
    let seg_w = rect.width() / items.len() as f32;
    let mut clicked = None;
    for (index, (label, tip, enabled)) in items.iter().enumerate() {
        let seg = egui::Rect::from_min_size(
            rect.min + Vec2::new(index as f32 * seg_w, 0.0),
            Vec2::new(seg_w, height),
        )
        .shrink(4.0);
        let sense = if *enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let mut response = ui.interact(seg, ui.id().with((id, index)), sense);
        if !tip.is_empty() {
            response = response.on_hover_text(*tip);
        }
        let (fill, fg) = if armed == Some(index) {
            (WARN, ON_ACCENT)
        } else if on == Some(index) {
            (ACCENT, ON_ACCENT)
        } else if response.hovered() && *enabled {
            (CARD_RAISED, TEXT)
        } else {
            (Color32::TRANSPARENT, if *enabled { TEXT } else { MUTED })
        };
        ui.painter().rect_filled(seg, Rounding::same(9.0), fill);
        ui.painter().text(
            seg.center(),
            Align2::CENTER_CENTER,
            *label,
            FontId::proportional(15.0),
            fg,
        );
        if response.clicked() {
            clicked = Some(index);
        }
    }
    clicked
}

fn primary_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).strong().color(ON_ACCENT))
        .fill(ACCENT)
        .min_size(Vec2::new(132.0, 36.0))
        .rounding(Rounding::same(10.0))
}

fn secondary_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).color(TEXT))
        .fill(CARD_RAISED)
        .min_size(Vec2::new(132.0, 36.0))
        .rounding(Rounding::same(10.0))
        .stroke(Stroke::new(1.0_f32, STROKE))
}

fn paint_gauge_card(
    painter: &egui::Painter,
    rect: egui::Rect,
    title: &str,
    reading: Reading,
    samples: &VecDeque<(f64, f32)>,
    note: Option<&str>,
) {
    painter.rect(
        rect,
        Rounding::same(16.0),
        CARD,
        Stroke::new(1.0_f32, STROKE),
    );
    let top = rect.left_top() + Vec2::new(18.0, 16.0);
    painter.text(
        top,
        Align2::LEFT_TOP,
        title,
        FontId::proportional(15.0),
        TEXT,
    );
    let kind = match reading {
        Reading::Temp(_) => "TEMPERATURE",
        Reading::Rpm(_) => "FAN SPEED",
    };
    painter.text(
        Pos2::new(rect.right() - 18.0, top.y + 2.0),
        Align2::RIGHT_TOP,
        note.unwrap_or(kind),
        FontId::proportional(11.0),
        MUTED,
    );
    let dial = egui::Rect::from_min_max(
        Pos2::new(rect.left() + 14.0, rect.top() + 40.0),
        Pos2::new(rect.right() - 14.0, rect.bottom() - 50.0),
    );
    let spark = egui::Rect::from_min_max(
        Pos2::new(rect.left() + 18.0, rect.bottom() - 40.0),
        Pos2::new(rect.right() - 18.0, rect.bottom() - 14.0),
    );
    match reading {
        Reading::Temp(celsius) => {
            gauge::temperature(painter, dial, celsius);
            let color = celsius.map_or(MUTED, gauge::temp_band);
            sparkline(painter, spark, samples, color, 30.0, 100.0);
        }
        Reading::Rpm(rpm) => {
            gauge::rpm(painter, dial, rpm);
            sparkline(painter, spark, samples, ACCENT, 0.0, gauge::RPM_MAX);
        }
    }
}

fn sparkline(
    painter: &egui::Painter,
    rect: egui::Rect,
    samples: &VecDeque<(f64, f32)>,
    color: Color32,
    y_lo: f32,
    y_hi: f32,
) {
    if samples.is_empty() || rect.width() <= 1.0 {
        return;
    }
    let t_min = samples.front().map(|(t, _)| *t).unwrap_or(0.0);
    let t_max = samples.back().map(|(t, _)| *t).unwrap_or(t_min);
    let span = (t_max - t_min).max(1.0);
    let mut y_min = y_lo;
    let mut y_max = y_hi;
    for (_, y) in samples {
        y_min = y_min.min(*y);
        y_max = y_max.max(*y);
    }
    if y_max - y_min < 1.0 {
        y_min -= 1.0;
        y_max += 1.0;
    }
    let mut pts = Vec::with_capacity(samples.len());
    for (t, y) in samples {
        let xn = ((*t - t_min) / span).clamp(0.0, 1.0) as f32;
        let yn = ((*y - y_min) / (y_max - y_min)).clamp(0.0, 1.0);
        pts.push(Pos2::new(
            rect.left() + xn * rect.width(),
            rect.bottom() - yn * rect.height(),
        ));
    }
    if pts.len() >= 2 {
        painter.add(egui::Shape::line(pts, Stroke::new(1.8_f32, color)));
    } else if let Some(point) = pts.last() {
        painter.add(egui::Shape::line(
            vec![
                Pos2::new(rect.left(), point.y),
                Pos2::new(rect.right(), point.y),
            ],
            Stroke::new(1.8_f32, color),
        ));
    }
}

fn battery_status(ui: &mut egui::Ui, battery: Option<Battery>) {
    let Some(battery) = battery else {
        ui.label(RichText::new("GetSystemPowerStatus failed.").color(MUTED));
        return;
    };
    let percent = battery.percent;
    ui.label(
        RichText::new(percent.map_or("—".to_string(), |p| format!("{p}%")))
            .size(34.0)
            .color(TEXT),
    );
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 10.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, Rounding::same(5.0), CARD_RAISED);
    if let Some(p) = percent {
        let mut fill = rect;
        fill.set_width(rect.width() * f32::from(p) / 100.0);
        ui.painter().rect_filled(fill, Rounding::same(5.0), ACCENT);
    }
    ui.add_space(6.0);
    let source = match battery.on_ac {
        Some(true) => "On AC power",
        Some(false) => "On battery",
        None => "Power source unknown",
    };
    let state = if battery.charging { " · charging" } else { "" };
    ui.label(
        RichText::new(format!("{source}{state}"))
            .size(13.0)
            .color(MUTED),
    );
}

fn firmware_charge_text(power: Option<&PowerSnapshot>) -> String {
    let Some(power) = power else {
        return "Firmware values not read yet.".to_string();
    };
    let policy = match power.charge_policy {
        Ok(0) => "Standard".to_string(),
        Ok(4) => "Custom".to_string(),
        Ok(other) => format!("policy {other}"),
        Err(_) => "policy unreadable".to_string(),
    };
    let stop = match power.charge_stop {
        Ok(stop) => format!("stop {stop}%"),
        Err(_) => "stop unreadable".to_string(),
    };
    format!("Firmware now: {policy}, {stop}.")
}

fn info_card(ui: &mut egui::Ui, title: &str, body: &str) {
    card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        section_title(ui, title);
        ui.add_space(4.0);
        ui.label(RichText::new(body).color(MUTED));
    });
}

fn banner(ui: &mut egui::Ui, text: &str, color: Color32) {
    Frame::none()
        .fill(Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            28,
        ))
        .rounding(Rounding::same(10.0))
        .stroke(Stroke::new(1.0_f32, color))
        .inner_margin(Margin::same(10.0))
        .show(ui, |ui| {
            ui.colored_label(color, text);
        });
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}

fn parse_temp(text: &str) -> Option<f32> {
    let number = text.trim().trim_end_matches("°C").trim();
    number.parse().ok()
}

fn parse_rpm(text: &str) -> Option<f32> {
    text.split_whitespace()
        .next()?
        .trim_end_matches('?')
        .parse()
        .ok()
}

#[cfg(test)]
mod background_tests {
    use super::*;

    fn app() -> AeroApp {
        let instance = crate::instance::Instance::acquire(true, true)
            .unwrap()
            .unwrap();
        AeroApp::new(BackendChoice::Mock, true, instance)
    }

    #[test]
    fn first_launch_setup_completes_through_the_app_without_opening_profiles() {
        let mut app = app();
        // Simulate the real launch eligibility while keeping task registration
        // and all hardware in the mock backend.
        app.elevated = true;
        app.setup_startup_once(true);
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !app.profiles.settings().startup_setup_complete {
            assert_eq!(app.poll_background(&ctx), BackgroundAction::Wait);
            assert!(Instant::now() < deadline, "{}", app.report);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(app.page, Page::Fans));
        assert!(!app.open_window_requested);
    }

    fn finish(app: &mut AeroApp, ctx: &egui::Context, mode: FanMode) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert_eq!(
                app.poll_background(ctx),
                BackgroundAction::Wait,
                "report: {}; desktop: {:?}",
                app.report,
                app.desktop.last_error()
            );
            if !app.busy
                && app.fan_apply.pending_mode.is_none()
                && app.fan_apply.active_mode == Some(mode)
            {
                break;
            }
            assert!(Instant::now() < deadline, "{}", app.report);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn background_start_and_resume_restore_gaming_without_opening_a_window() {
        let mut app = app();
        let ctx = egui::Context::default();
        let gaming = aero_core::builtin_config()
            .profile("Gaming")
            .unwrap()
            .clone();
        app.profiles.remember_fan(gaming, None).unwrap();
        assert!(!app.profiles.settings().keep_in_tray);
        finish(&mut app, &ctx, FanMode::Gaming);
        assert!(app.tray_requested);
        assert_eq!(app.last_gpu_request, Some(true));
        assert!(!app.open_window_requested);

        // Simulate firmware reset independently of the saved user preference.
        app.session.apply(FanMode::Normal, 0, false);
        finish(&mut app, &ctx, FanMode::Normal);
        assert_eq!(
            app.profiles.fan_restore_profile().unwrap().fan_mode,
            FanMode::Gaming
        );
        app.desktop
            .inject_event(crate::desktop_events::DesktopEvent::Resume);
        finish(&mut app, &ctx, FanMode::Gaming);
        assert!(!app.open_window_requested);

        app.desktop
            .inject_event(crate::desktop_events::DesktopEvent::Show);
        assert_eq!(app.poll_background(&ctx), BackgroundAction::Open);
        // Opening uses this existing session; it does not rerun restoration.
        assert!(app.startup_checked);
        assert!(app.queued_profile.is_none());
    }

    #[test]
    fn background_exit_and_errors_do_not_leave_an_inaccessible_controller() {
        let mut app = app();
        let ctx = egui::Context::default();
        finish(&mut app, &ctx, FanMode::Normal);
        app.report_error = true;
        assert_eq!(app.poll_background(&ctx), BackgroundAction::Open);
        app.desktop
            .inject_event(crate::desktop_events::DesktopEvent::Exit);
        assert_eq!(app.poll_background(&ctx), BackgroundAction::Exit);
    }

    #[test]
    #[cfg(windows)]
    #[ignore = "Requires an interactive Windows desktop for the real notification icon"]
    fn native_tray_keeps_mock_restoration_accessible_without_a_control_window() {
        let mut app = app();
        // Exercise the real desktop/tray integration, with all hardware and
        // saved settings still isolated in the mock backend.
        app.desktop = crate::desktop_events::DesktopEvents::new(false);
        let ctx = egui::Context::default();
        let gaming = aero_core::builtin_config()
            .profile("Gaming")
            .unwrap()
            .clone();
        app.profiles.remember_fan(gaming, None).unwrap();
        finish(&mut app, &ctx, FanMode::Gaming);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.desktop.can_hide_to_tray() {
            assert_eq!(app.poll_background(&ctx), BackgroundAction::Wait);
            assert!(Instant::now() < deadline, "{:?}", app.desktop.last_error());
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!app.open_window_requested);
        app.desktop
            .inject_event(crate::desktop_events::DesktopEvent::Show);
        assert_eq!(app.poll_background(&ctx), BackgroundAction::Open);
    }
}
