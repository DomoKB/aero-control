//! Status view and curve editor.
//!
//! On Windows the default backend is `WindowsWriter`. `--mock` and non-Windows
//! builds keep the fake controller. Sensor reads are display only and never set a duty.

use std::collections::VecDeque;
use std::time::Instant;

use aero_core::{
    constrain_drag, insert_point, issues, parse_custom_fan_table_text, remove_point,
    stair_polyline, CurveIssue, CurvePoint, FanCurve, FanMode,
};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Frame, Layout, Margin, Pos2, RichText, Rounding, Sense,
    Stroke, TextStyle, Vec2,
};

use crate::session::{BackendChoice, FanSession, SensorSnapshot};

mod keyboard;
use keyboard::KeyboardTab;

const PLOT_X_END: f64 = 100.0;
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
const COOL: Color32 = Color32::from_rgb(88, 186, 214);
const AMBER: Color32 = Color32::from_rgb(232, 184, 74);
const ORANGE: Color32 = Color32::from_rgb(232, 122, 58);
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
    fixed_percent: u8,
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
        let keyboard = KeyboardTab::new(matches!(choice, BackendChoice::Mock));

        let points = aero_core::factory_curve().points().to_vec();
        let issues = issues(&points);
        let session = FanSession::start(choice);
        let report = session.banner.clone().unwrap_or_else(|| {
            format!(
                "{} backend. Log: {}",
                session.backend_label, session.log_path
            )
        });
        let mut sensors = Sensors {
            cpu: "—".into(),
            gpu: "—".into(),
            gpu2: "—".into(),
            rpm1: "—".into(),
            rpm2: "—".into(),
            flags: "—".into(),
        };
        let mut sensor_error = None;
        let mut history = History::new();
        if let Some(snapshot) = session.initial_sensors.clone() {
            sensor_error = snapshot.error.clone();
            sensors.cpu = snapshot.cpu;
            sensors.gpu = snapshot.gpu;
            sensors.gpu2 = snapshot.gpu2;
            sensors.rpm1 = snapshot.rpm1;
            sensors.rpm2 = snapshot.rpm2;
            sensors.flags = snapshot.flags;
            history.cpu.push(0.0, parse_temp(&sensors.cpu));
            history.gpu.push(0.0, parse_temp(&sensors.gpu));
            history.rpm1.push(0.0, parse_rpm(&sensors.rpm1));
            history.rpm2.push(0.0, parse_rpm(&sensors.rpm2));
        }
        let active_mode = mode_from_flags(&sensors.flags);
        Self {
            session,
            busy: false,
            report_error: false,
            sensor_error,
            arm_max_test: false,
            page: Page::Fans,
            points,
            issues,
            drag_index: None,
            fixed_percent: 50,
            expert: false,
            arm_max: false,
            import_text: "45,35;60,45;0,0;85,80;90,100".to_string(),
            import_note: String::new(),
            report,
            sensors,
            history,
            started: Instant::now(),
            active_mode,
            pending_mode: None,
            keyboard,
        }
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
        self.busy = true;
        self.pending_mode = Some(mode);
        self.session.apply(mode, raw, self.expert);
        self.arm_max = false;
        self.arm_max_test = false;
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
                self.session.upload(curve.points().to_vec());
            }
            Err(issues) => {
                self.issues = issues;
                self.report = "Upload stayed disabled: the curve is not valid.".to_string();
                self.report_error = true;
            }
        }
    }

    fn reset_factory(&mut self) {
        self.points = aero_core::factory_curve().points().to_vec();
        self.drag_index = None;
        self.recompute();
        self.report = "Editor loaded the YB factory curve. Nothing is sent until Upload.".into();
        self.report_error = false;
    }
}

impl eframe::App for AeroApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_session();
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
        ui.label(RichText::new("Fans").size(22.0).strong().color(TEXT));
        ui.label(RichText::new("Live readings. The firmware still runs the fans.").color(MUTED));
        ui.add_space(14.0);
        if let Some(error) = &self.sensor_error {
            banner(ui, error, DANGER);
            ui.add_space(10.0);
        }
        self.metric_row(ui);
        ui.add_space(8.0);
        ui.label(RichText::new(format!("GPU sensor 2  {}", self.sensors.gpu2)).color(MUTED));
        ui.add_space(18.0);
        ui.label(RichText::new("Fan mode").size(16.0).strong().color(TEXT));
        ui.add_space(8.0);
        self.mode_row(ui);
        ui.add_space(14.0);
        self.fixed_card(ui);
        ui.add_space(12.0);
        ui.label(RichText::new(&self.sensors.flags).size(13.0).color(MUTED));
    }

    fn metric_row(&self, ui: &mut egui::Ui) {
        let gap = 12.0;
        let card_h = 148.0;
        let width = ui.available_width();
        let cols: usize = if width > 980.0 {
            4
        } else if width > 560.0 {
            2
        } else {
            1
        };
        let col_w = ((width - gap * (cols as f32 - 1.0)) / cols as f32).max(140.0);
        let cpu_c = temp_color(parse_temp(&self.sensors.cpu));
        let gpu_c = temp_color(parse_temp(&self.sensors.gpu));
        let cards = [
            (
                "CPU",
                self.sensors.cpu.as_str(),
                cpu_c,
                &self.history.cpu,
                30.0_f32,
                100.0_f32,
            ),
            (
                "GPU",
                self.sensors.gpu.as_str(),
                gpu_c,
                &self.history.gpu,
                30.0_f32,
                100.0_f32,
            ),
            (
                "Fan 1",
                self.sensors.rpm1.as_str(),
                ACCENT,
                &self.history.rpm1,
                0.0_f32,
                5000.0_f32,
            ),
            (
                "Fan 2",
                self.sensors.rpm2.as_str(),
                ACCENT,
                &self.history.rpm2,
                0.0_f32,
                5000.0_f32,
            ),
        ];
        let rows = cards.len().div_ceil(cols);
        let origin = ui.cursor().min;
        for (index, (title, raw, color, trace, y_lo, y_hi)) in cards.into_iter().enumerate() {
            let col = index % cols;
            let row = index / cols;
            let rect = egui::Rect::from_min_size(
                origin + Vec2::new(col as f32 * (col_w + gap), row as f32 * (card_h + gap)),
                Vec2::new(col_w, card_h),
            );
            paint_metric(ui, rect, title, raw, color, &trace.samples, y_lo, y_hi);
        }
        let total_h = rows as f32 * card_h + (rows.saturating_sub(1) as f32) * gap;
        ui.add_space(total_h);
    }

    fn mode_row(&mut self, ui: &mut egui::Ui) {
        let allow = self.writes_allowed();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            self.mode_tile(
                ui,
                "Normal",
                "Firmware automatic control.",
                FanMode::Normal,
                allow,
                false,
            );
            self.mode_tile(
                ui,
                "Quiet",
                "Super Quiet. The flag sequence is still provisional.",
                FanMode::Quiet,
                allow,
                false,
            );
            self.mode_tile(
                ui,
                "Gaming",
                "Fan writes currently match Normal. Still provisional.",
                FanMode::Gaming,
                allow,
                false,
            );
            self.mode_tile(
                ui,
                "Fixed",
                "One duty for both fans. Floor is 35% unless expert is on.",
                FanMode::Fixed,
                allow,
                false,
            );
            let max_label = if self.arm_max { "Confirm Max" } else { "Max" };
            self.mode_tile(
                ui,
                max_label,
                "Both fans at raw 229 (100%). Click twice.",
                FanMode::Max,
                allow,
                self.arm_max,
            );
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    allow,
                    egui::Button::new(RichText::new("Restore auto").color(TEXT))
                        .fill(CARD_RAISED)
                        .min_size(Vec2::new(140.0, 36.0))
                        .rounding(Rounding::same(10.0)),
                )
                .on_hover_text("Sends the Normal sequence: fixed off, step off, auto on.")
                .clicked()
            {
                self.apply_firmware(FanMode::Normal);
            }
            let test_label = if self.arm_max_test {
                "Confirm: Max for 10 s"
            } else {
                "Test: Max for 10 s"
            };
            if ui
                .add_enabled(
                    allow,
                    egui::Button::new(RichText::new(test_label).color(TEXT))
                        .fill(CARD_RAISED)
                        .min_size(Vec2::new(180.0, 36.0))
                        .rounding(Rounding::same(10.0))
                        .stroke(Stroke::new(1.0_f32, WARN)),
                )
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
        });
    }

    fn mode_tile(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        tip: &str,
        mode: FanMode,
        enabled: bool,
        armed: bool,
    ) {
        let on = self.active_mode == Some(mode) || (armed && mode == FanMode::Max);
        let fill = if armed && mode == FanMode::Max {
            WARN
        } else if on {
            ACCENT
        } else {
            CARD_RAISED
        };
        let fg = if on || (armed && mode == FanMode::Max) {
            ON_ACCENT
        } else {
            TEXT
        };
        let button = egui::Button::new(RichText::new(label).size(16.0).strong().color(fg))
            .fill(fill)
            .min_size(Vec2::new(128.0, 64.0))
            .rounding(Rounding::same(12.0))
            .stroke(if on {
                Stroke::NONE
            } else {
                Stroke::new(1.0_f32, STROKE)
            });
        if ui.add_enabled(enabled, button).on_hover_text(tip).clicked() {
            if mode == FanMode::Max && !self.arm_max {
                self.arm_max = true;
                self.report = "Max sets both fans to raw 229 (100%). Click Confirm Max.".into();
                self.report_error = false;
            } else if mode == FanMode::Fixed {
                self.apply_firmware(FanMode::Fixed);
            } else {
                self.apply_firmware(mode);
            }
        }
    }

    fn fixed_card(&mut self, ui: &mut egui::Ui) {
        Frame::none()
            .fill(CARD)
            .rounding(Rounding::same(12.0))
            .stroke(Stroke::new(1.0_f32, STROKE))
            .inner_margin(Margin::same(14.0))
            .show(ui, |ui| {
                ui.label(RichText::new("Fixed duty").strong().color(TEXT));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Slider::new(&mut self.fixed_percent, 0..=100)
                            .suffix("%")
                            .trailing_fill(true),
                    );
                    if ui
                        .add_enabled(
                            self.writes_allowed(),
                            egui::Button::new(RichText::new("Apply fixed").color(ON_ACCENT))
                                .fill(ACCENT)
                                .min_size(Vec2::new(120.0, 32.0))
                                .rounding(Rounding::same(10.0)),
                        )
                        .on_hover_text(
                            "Sends this duty to both fans. Stays at or above 35% (raw 80) unless expert is on.",
                        )
                        .clicked()
                    {
                        self.apply_firmware(FanMode::Fixed);
                    }
                    ui.checkbox(&mut self.expert, "Expert floor override")
                        .on_hover_text("Allows a fixed duty below 35% (raw 80).");
                });
            });
    }

    fn curve_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Curve").size(22.0).strong().color(TEXT));
        ui.label(RichText::new("YB factory curve. Edit it here, then upload.").color(MUTED));
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(RichText::new("Reset to Factory").color(TEXT))
                        .fill(CARD_RAISED)
                        .min_size(Vec2::new(160.0, 36.0))
                        .rounding(Rounding::same(10.0))
                        .stroke(Stroke::new(1.0_f32, STROKE)),
                )
                .on_hover_text(
                    "Loads the YB factory curve into the editor. Nothing is sent until Upload.",
                )
                .clicked()
            {
                self.reset_factory();
            }
            let valid = self.issues.is_empty();
            if ui
                .add_enabled(
                    valid && self.writes_allowed(),
                    egui::Button::new(RichText::new("Upload").size(16.0).strong().color(ON_ACCENT))
                        .fill(ACCENT)
                        .min_size(Vec2::new(140.0, 36.0))
                        .rounding(Rounding::same(10.0)),
                )
                .on_hover_text(
                    "Writes all 15 slots and reads them back. A mismatch retries once, then restores firmware auto. Percents round half-up: 30% is raw 69, while this chip's table stores Control Center's truncated raw 68.",
                )
                .clicked()
            {
                self.upload();
            }
            if self.issues.is_empty() {
                ui.colored_label(ACCENT, "Valid");
            }
        });
        if !self.issues.is_empty() {
            ui.add_space(6.0);
            for issue in &self.issues {
                let where_ = issue
                    .index
                    .map(|index| format!("point {index}: "))
                    .unwrap_or_default();
                ui.colored_label(DANGER, format!("{where_}{}", issue.message));
            }
        }
        ui.add_space(10.0);
        self.plot(ui);
        ui.add_space(6.0);
        egui::CollapsingHeader::new("How to edit")
            .default_open(false)
            .show(ui, |ui| {
                ui.label("Drag a point. Double-click the plot to add one (max 15). Right-click a point to remove it (min 2). The stair step holds each duty until the next temperature. The vertical line is the current CPU temperature.");
            });
        ui.add_space(8.0);
        self.table(ui);
        ui.add_space(8.0);
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

    fn plot(&mut self, ui: &mut egui::Ui) {
        let percent_points: Vec<(f64, f64)> = self
            .points
            .iter()
            .map(|point| {
                (
                    f64::from(point.temp_c),
                    f64::from(aero_core::raw_to_percent(point.duty_raw)),
                )
            })
            .collect();
        let stair = stair_polyline(&percent_points, PLOT_X_END);
        let stair_line: egui_plot::PlotPoints = stair.iter().map(|(x, y)| [*x, *y]).collect();
        let markers: egui_plot::PlotPoints = percent_points.iter().map(|(x, y)| [*x, *y]).collect();
        let selected: Option<[f64; 2]> = self
            .drag_index
            .and_then(|index| percent_points.get(index).map(|(x, y)| [*x, *y]));
        let cpu_now = parse_temp(&self.sensors.cpu).map(f64::from);
        let accent = ACCENT;
        let hot = temp_color(parse_temp(&self.sensors.cpu));

        let response = egui_plot::Plot::new("fan_curve")
            .height(380.0)
            .allow_drag(false)
            .allow_scroll(false)
            .allow_zoom(false)
            .allow_boxed_zoom(false)
            .show_background(true)
            .include_x(0.0)
            .include_x(PLOT_X_END)
            .include_y(0.0)
            .include_y(100.0)
            .x_axis_label("Temperature (°C)")
            .y_axis_label("Duty (%)")
            .legend(egui_plot::Legend::default())
            .show(ui, |plot_ui| {
                plot_ui.line(
                    egui_plot::Line::new(stair_line)
                        .name("Stair step")
                        .color(accent)
                        .width(2.5_f32),
                );
                plot_ui.points(
                    egui_plot::Points::new(markers)
                        .name("Points")
                        .color(Color32::from_rgb(236, 240, 244))
                        .radius(5.0_f32),
                );
                if let Some(point) = selected {
                    plot_ui.points(
                        egui_plot::Points::new(vec![point])
                            .name("Selected")
                            .color(accent)
                            .radius(9.0_f32),
                    );
                }
                if let Some(x) = cpu_now {
                    plot_ui.vline(
                        egui_plot::VLine::new(x)
                            .name("CPU")
                            .color(hot)
                            .width(1.5_f32),
                    );
                }
                plot_ui.pointer_coordinate()
            });
        self.handle_plot(&response);
    }

    fn handle_plot(&mut self, response: &egui_plot::PlotResponse<Option<egui_plot::PlotPoint>>) {
        let Some(pointer) = response.inner else {
            if response.response.drag_stopped() {
                self.drag_index = None;
            }
            return;
        };
        if response.response.drag_started()
            || (response.response.dragged() && self.drag_index.is_none())
        {
            self.drag_index = nearest(&self.points, pointer.x, pointer.y);
        }
        if response.response.dragged() {
            if let Some(index) = self.drag_index {
                if let Some(point) = constrain_drag(
                    &self.points,
                    index,
                    pointer.x.round() as i32,
                    pointer.y.round() as i32,
                ) {
                    self.points[index] = point;
                    self.recompute();
                }
            }
        }
        if response.response.drag_stopped() {
            self.drag_index = None;
        }
        if response.response.double_clicked() {
            let temp = pointer.x.round().clamp(30.0, 95.0) as u8;
            let duty = pointer.y.round().clamp(0.0, 100.0) as u8;
            if insert_point(&mut self.points, temp, duty).is_ok() {
                self.recompute();
            }
        }
        if response.response.secondary_clicked() {
            if let Some(index) = nearest(&self.points, pointer.x, pointer.y) {
                if remove_point(&mut self.points, index).is_ok() {
                    self.recompute();
                }
            }
        }
    }

    fn table(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                egui::Grid::new("points")
                    .striped(true)
                    .spacing([18.0, 6.0])
                    .show(ui, |ui| {
                        ui.strong("Index");
                        ui.strong("°C");
                        ui.strong("Duty %");
                        ui.strong("Raw");
                        ui.end_row();
                        for (index, point) in self.points.iter_mut().enumerate() {
                            ui.label(index.to_string());
                            let mut temp = point.temp_c;
                            let mut percent = aero_core::raw_to_percent(point.duty_raw);
                            let mut raw = point.duty_raw;
                            if ui
                                .add(egui::DragValue::new(&mut temp).range(0..=100))
                                .changed()
                            {
                                point.temp_c = temp;
                            }
                            if ui
                                .add(egui::DragValue::new(&mut percent).range(0..=100))
                                .changed()
                            {
                                point.duty_raw = aero_core::percent_to_raw(percent);
                            }
                            if ui
                                .add(egui::DragValue::new(&mut raw).range(0..=229))
                                .changed()
                            {
                                point.duty_raw = raw;
                            }
                            ui.end_row();
                        }
                    });
            });
        self.recompute();
    }

    fn power_ui(&self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Power").size(22.0).strong().color(TEXT));
        ui.add_space(8.0);
        info_card(
            ui,
            "Not sent to the EC",
            "Dynamic Boost, NvPowerConfig, the NVIDIA thermal target, AI Boost, and CPU PL1/PL2 can sit on a TOML profile. Applying a profile does not write them.",
        );
        ui.add_space(12.0);
        info_card(ui, "Firmware flags", &self.sensors.flags);
    }

    fn battery_ui(&self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Battery").size(22.0).strong().color(TEXT));
        ui.add_space(8.0);
        info_card(
            ui,
            "Charge limit",
            "A profile may store charge_limit. Nothing in this build writes a battery or charge policy to the embedded controller.",
        );
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

fn paint_metric(
    ui: &egui::Ui,
    rect: egui::Rect,
    title: &str,
    raw: &str,
    color: Color32,
    samples: &VecDeque<(f64, f32)>,
    y_lo: f32,
    y_hi: f32,
) {
    let painter = ui.painter();
    painter.rect(
        rect,
        Rounding::same(14.0),
        CARD,
        Stroke::new(1.0_f32, STROKE),
    );
    let (number, unit) = split_metric(raw);
    let number_pos = rect.left_top() + Vec2::new(16.0, 36.0);
    painter.text(
        rect.left_top() + Vec2::new(16.0, 12.0),
        Align2::LEFT_TOP,
        title,
        FontId::proportional(13.0),
        MUTED,
    );
    let number_galley = painter.layout_no_wrap(number, FontId::proportional(34.0), color);
    painter.galley(number_pos, number_galley.clone(), color);
    if !unit.is_empty() {
        painter.text(
            number_pos + Vec2::new(number_galley.size().x + 8.0, 16.0),
            Align2::LEFT_TOP,
            unit,
            FontId::proportional(14.0),
            MUTED,
        );
    }
    let spark = egui::Rect::from_min_max(
        Pos2::new(rect.left() + 16.0, rect.bottom() - 46.0),
        Pos2::new(rect.right() - 16.0, rect.bottom() - 14.0),
    );
    sparkline(painter, spark, samples, color, y_lo, y_hi);
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

fn info_card(ui: &mut egui::Ui, title: &str, body: &str) {
    Frame::none()
        .fill(CARD)
        .rounding(Rounding::same(12.0))
        .stroke(Stroke::new(1.0_f32, STROKE))
        .inner_margin(Margin::same(14.0))
        .show(ui, |ui| {
            ui.label(RichText::new(title).strong().color(TEXT));
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

fn temp_color(celsius: Option<f32>) -> Color32 {
    let Some(c) = celsius else {
        return MUTED;
    };
    if c < 55.0 {
        mix(COOL, AMBER, (c - 40.0) / 15.0)
    } else if c < 75.0 {
        mix(AMBER, ORANGE, (c - 55.0) / 20.0)
    } else {
        mix(ORANGE, HOT, (c - 75.0) / 15.0)
    }
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

fn split_metric(raw: &str) -> (String, String) {
    let raw = raw.trim();
    if let Some(number) = raw.strip_suffix("°C") {
        return (number.trim().to_string(), "°C".into());
    }
    if let Some(number) = raw.split_whitespace().next() {
        if number.trim_end_matches('?').parse::<f32>().is_ok() {
            let unit = raw[number.len()..].trim();
            return (number.trim_end_matches('?').to_string(), unit.to_string());
        }
    }
    (raw.to_string(), String::new())
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

fn nearest(points: &[CurvePoint], x: f64, y: f64) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| {
            let dl = dist(left, x, y);
            let dr = dist(right, x, y);
            dl.partial_cmp(&dr).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(index, _)| index)
}

fn dist(point: &CurvePoint, x: f64, y: f64) -> f64 {
    let dx = (f64::from(point.temp_c) - x) / 80.0;
    let dy = (f64::from(aero_core::raw_to_percent(point.duty_raw)) - y) / 100.0;
    dx * dx + dy * dy
}
