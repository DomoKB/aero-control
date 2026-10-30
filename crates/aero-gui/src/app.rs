//! Status view and curve editor.
//!
//! On Windows the default backend is `WindowsWriter`. `--mock` and non-Windows
//! builds keep the fake controller. Sensor reads are display only and never set a duty.

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

mod curve_chart;
mod gauge;
mod kbd_preview;
mod keyboard;
mod live_duty;
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
const MUTED: Color32 = Color32::from_rgb(148, 160, 176);
const ACCENT: Color32 = Color32::from_rgb(64, 196, 168);
const ON_ACCENT: Color32 = Color32::from_rgb(10, 24, 22);
const WARN: Color32 = Color32::from_rgb(214, 154, 72);
const DANGER: Color32 = Color32::from_rgb(214, 96, 88);
const AMBER: Color32 = Color32::from_rgb(232, 184, 74);
const HOT: Color32 = Color32::from_rgb(220, 72, 64);

pub struct AeroApp {
    session: FanSession,
    busy: bool,
    report_error: bool,
    sensor_error: Option<String>,
    arm_max_test: bool,
    page: Page,
    points: Vec<CurvePoint>,
    issues: Vec<CurveIssue>,
    drag_index: Option<usize>,
    applied_curve: Vec<CurvePoint>,
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
    active_mode: Option<FanMode>,
    pending_mode: Option<FanMode>,
    keyboard: KeyboardTab,
    firmware_power: Option<PowerSnapshot>,
    charge_custom: bool,
    charge_stop: u8,
    system: SystemPower,
    plans: Vec<Plan>,
    active_plan: Result<Plan, String>,
    battery: Option<Battery>,
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
        "Quiet: Super Quiet. The flag sequence is still provisional.",
    ),
    (
        "Gaming",
        FanMode::Gaming,
        "Gaming: fan writes currently match Normal. Still provisional.",
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
    pub fn new(cc: &eframe::CreationContext<'_>, choice: BackendChoice) -> Self {
        install_theme(&cc.egui_ctx);
        let mock = matches!(choice, BackendChoice::Mock);
        let points = aero_core::factory_curve().points().to_vec();
        let issues = issues(&points);
        let session = FanSession::start(choice);
        let report = session.banner.clone().unwrap_or_else(|| {
            format!(
                "{} backend. Log: {}",
                session.backend_label, session.log_path
            )
        });
        let initial = session.initial_sensors.clone();
        let system = SystemPower::new(mock);
        let mut app = Self {
            session,
            busy: false,
            report_error: false,
            sensor_error: None,
            arm_max_test: false,
            page: Page::Fans,
            points: points.clone(),
            issues,
            drag_index: None,
            applied_curve: points,
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
            active_mode: None,
            pending_mode: None,
            keyboard: KeyboardTab::new(mock),
            firmware_power: None,
            charge_custom: false,
            charge_stop: 80,
            plans: system.plans(),
            active_plan: system.active(),
            battery: system.battery(),
            system,
        };
        if let Some(snapshot) = initial {
            app.apply_snapshot(snapshot);
        }
        app
    }

    fn poll_session(&mut self) {
        while let Some(event) = self.session.poll() {
            self.busy = event.busy;
            if let Some(snapshot) = event.sensors {
                self.apply_snapshot(snapshot);
            }
            if let Some(message) = event.message {
                if event.error {
                    self.pending_mode = None;
                } else if let Some(mode) = self.pending_mode.take() {
                    self.active_mode = Some(mode);
                }
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
        if self.active_mode.is_none() && self.pending_mode.is_none() {
            self.active_mode = mode_from_flags(&self.sensors.flags);
        }
    }

    fn recompute(&mut self) {
        self.issues = issues(&self.points);
    }

    fn writes_allowed(&self) -> bool {
        self.session.writes_enabled && !self.busy
    }

    fn apply_firmware(&mut self, mode: FanMode) {
        if !self.writes_allowed() {
            self.report = if self.busy {
                "A firmware command is already running.".to_string()
            } else {
                "Fan writes are off. Run aero-gui as Administrator.".to_string()
            };
            self.report_error = true;
            return;
        }
        let raw = aero_core::percent_to_raw(self.fixed_percent);
        if mode == FanMode::Fixed {
            self.live_duty.applied(self.fixed_percent);
        }
        self.busy = true;
        self.pending_mode = Some(mode);
        self.session.apply(mode, raw, self.expert);
        self.arm_max = false;
        self.arm_max_test = false;
    }

    fn fixed_shown(&self) -> bool {
        self.pending_mode.or(self.active_mode) == Some(FanMode::Fixed)
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
        if self.active_mode != Some(FanMode::Fixed) || self.pending_mode.is_some() {
            self.apply_firmware(FanMode::Fixed);
            return;
        }
        self.live_duty.applied(self.fixed_percent);
        self.busy = true;
        self.pending_mode = Some(FanMode::Fixed);
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
                self.busy = true;
                self.pending_mode = Some(FanMode::Custom);
                self.applied_curve = curve.points().to_vec();
                self.session.upload(self.applied_curve.clone());
            }
            Err(issues) => {
                self.issues = issues;
                self.report = "Upload stayed disabled: the curve is not valid.".to_string();
                self.report_error = true;
            }
        }
    }

    fn push_curve(&mut self) {
        if self.active_mode == Some(FanMode::Custom)
            && self.pending_mode.is_none()
            && self.drag_index.is_none()
            && self.issues.is_empty()
            && self.writes_allowed()
            && self.points != self.applied_curve
        {
            self.applied_curve = self.points.clone();
            self.busy = true;
            self.pending_mode = Some(FanMode::Custom);
            self.session.update_curve(self.applied_curve.clone());
        }
    }

    fn reset_factory(&mut self) {
        self.points = aero_core::factory_curve().points().to_vec();
        self.drag_index = None;
        self.recompute();
        self.report = "Editor loaded the YB factory curve.".into();
        self.report_error = false;
    }
}

impl eframe::App for AeroApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_session();
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
            ui.label(RichText::new("AERO 17 YB").size(13.0).color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (admin, admin_bg) = if self.session.backend_label == "REAL" {
                    if self.session.writes_enabled {
                        ("Administrator", ACCENT)
                    } else {
                        ("Not elevated", WARN)
                    }
                } else {
                    ("Simulated", Color32::from_rgb(72, 96, 140))
                };
                pill(
                    ui,
                    admin,
                    admin_bg,
                    if admin_bg == WARN {
                        ON_ACCENT
                    } else {
                        ON_ACCENT
                    },
                );
                ui.add_space(8.0);
                let (name, bg) = if self.session.backend_label == "REAL" {
                    ("REAL", ACCENT)
                } else {
                    ("MOCK", Color32::from_rgb(96, 150, 214))
                };
                pill(ui, name, bg, ON_ACCENT);
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
        ui.label(RichText::new("SECTIONS").size(11.0).color(MUTED));
        ui.add_space(8.0);
        for (label, page) in [
            ("Fans", Page::Fans),
            ("Curve", Page::Curve),
            ("Power", Page::Power),
            ("Battery", Page::Battery),
            ("Keyboard", Page::Keyboard),
            ("Log", Page::Log),
        ] {
            nav_item(ui, label, self.page == page, &mut self.page, page);
            ui.add_space(4.0);
        }
    }

    fn fans_ui(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Fans",
            "Live readings. The firmware still runs the fans.",
        );
        if let Some(error) = &self.sensor_error {
            banner(ui, error, DANGER);
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
                        .on_hover_text("Sends the Normal sequence: fixed off, step off, auto on.")
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
                .find(|(_, mode, _)| self.active_mode == Some(*mode))
                .map_or(
                    "Mode unknown until the firmware flags are read.",
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
            .position(|(_, mode, _)| self.active_mode == Some(*mode));
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
            "One firmware table drives both fans. Each duty holds until the next point.",
        );
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                section_title(ui, "Custom curve");
                ui.add_space(8.0);
                self.curve_state(ui);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.active_mode != Some(FanMode::Custom)
                        && ui
                            .add_enabled(
                                self.issues.is_empty() && self.writes_allowed(),
                                primary_button("Use this curve"),
                            )
                            .on_hover_text(
                                "Switches the fans to this curve: writes all 15 slots and reads them back. A mismatch retries once, then restores firmware auto.",
                            )
                            .clicked()
                    {
                        self.upload();
                    }
                    if ui
                        .add(secondary_button("Factory"))
                        .on_hover_text("Loads the YB factory curve from Control Center.")
                        .clicked()
                    {
                        self.reset_factory();
                    }
                });
            });
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
            let height = (ui.available_height() - 64.0).clamp(240.0, 460.0);
            curve_chart::show(ui, &mut self.points, &mut self.drag_index, &markers, height);
            self.recompute();
            ui.add_space(6.0);
            ui.label(
                RichText::new("Drag a point to move it · double-click to add (max 15) · right-click to remove (min 2)")
                    .size(13.0)
                    .color(MUTED),
            );
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
        let (text, fill) = if self.busy && self.pending_mode == Some(FanMode::Custom) {
            ("Applying…", AMBER)
        } else if self.active_mode == Some(FanMode::Custom) {
            if self.issues.is_empty() {
                ("Active · edits apply on release", ACCENT)
            } else {
                ("Active · fix the curve to apply", WARN)
            }
        } else {
            ("Not active", CARD_RAISED)
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
            "Windows power plan and the NVIDIA power config switch.",
        );
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
            section_title(ui, "NVIDIA power config");
            ui.add_space(10.0);
            let on = match current {
                Some(Ok(0)) => Some(0),
                Some(Ok(1)) => Some(1),
                _ => None,
            };
            let items = [("Off (0)", "", allow), ("On (1)", "", allow)];
            if let Some(index) = segmented(ui, "nv_power", &items, on, None) {
                self.send_power(PowerWrite::NvPowerConfig(index == 1));
            }
            ui.add_space(8.0);
            let reading = match current {
                Some(Ok(value)) => format!("GetNvPowerConfig reads {value}."),
                Some(Err(err)) => format!("GetNvPowerConfig failed: {err}"),
                None => "GetNvPowerConfig has not been read.".to_string(),
            };
            ui.label(RichText::new(reading).size(13.0).color(MUTED));
            ui.label(
                RichText::new(
                    "Sends SetNvPowerConfig with Data 0 or 1, the values Control Center writes. Its effect on GPU power is not measured here.",
                )
                .size(13.0)
                .color(MUTED),
            );
        });
        ui.add_space(16.0);
        info_card(
            ui,
            "Not controlled here",
            "CPU PL1/PL2, Dynamic Boost, the NVIDIA thermal target and the GPU MUX. A profile can store some of these, but nothing in this build writes them.",
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
                section_title(ui, "Battery health code");
                ui.add_space(8.0);
                let code = match power.as_ref().map(|power| &power.battery_health) {
                    Some(Ok(code)) => code.to_string(),
                    Some(Err(_)) => "error".to_string(),
                    None => "—".to_string(),
                };
                ui.label(RichText::new(code).size(34.0).color(TEXT));
                ui.label(
                    RichText::new(
                        "Raw GetBatteryHealth value. Gigabyte does not document what the code means.",
                    )
                    .size(13.0)
                    .color(MUTED),
                );
            });
        });
        ui.add_space(16.0);
        let allow = self.writes_allowed();
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, "Charging policy");
            ui.add_space(10.0);
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
            self.report = "Battery and power writes need an elevated aero-gui.".to_string();
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
                ui.label("The firmware runs the fans. Sensor reads are display only and never set a duty. Thermal shutdown still belongs to the CPU, the GPU, and the embedded controller (PROCHOT and GPU slowdown).");
                ui.add_space(6.0);
                ui.label("Curve upload writes all 15 slots, reads them back, and restores firmware auto on a mismatch or a panic. Closing the window does the same if the last command was Fixed or Max.");
                ui.add_space(6.0);
                ui.label("The editor starts from Control Center's YB factory curve (DeepFan setDefault_Click, confirmed by GetFanIndexValue on this P77YB). Quiet and Gaming flag sequences are still provisional. Upload rounds percent half-up, so 30% is raw 69 here and raw 68 on the chip, which truncates.");
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

fn mode_from_flags(flags: &str) -> Option<FanMode> {
    let value = |key: &str| {
        flags.split(" · ").find_map(|part| {
            part.strip_prefix(key)
                .map(str::trim)
                .filter(|rest| !rest.is_empty())
        })
    };
    if value("super quiet") == Some("1") {
        Some(FanMode::Quiet)
    } else if value("auto") == Some("1") {
        Some(FanMode::Normal)
    } else {
        None
    }
}
