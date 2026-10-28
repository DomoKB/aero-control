//! Status view and curve editor.
//!
//! On Windows the default backend is `WindowsWriter`. `--mock` and non-Windows
//! builds keep the fake controller. Sensor refreshes only call getters.

use aero_core::{
    constrain_drag, insert_point, issues, parse_custom_fan_table_text, presets, remove_point,
    stair_polyline, CurveIssue, CurvePoint, FanCurve, FanMode, PresetKind,
};
use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints, Points, VLine};

use crate::session::{BackendChoice, FanSession, SensorSnapshot};

const PLOT_X_END: f64 = 100.0;

pub struct AeroApp {
    session: FanSession,
    busy: bool,
    report_error: bool,
    sensor_error: Option<String>,
    arm_max_test: bool,
    tab: Tab,
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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Status,
    Curve,
}

struct Sensors {
    cpu: String,
    gpu: String,
    gpu2: String,
    rpm1: String,
    rpm2: String,
    flags: String,
}

impl AeroApp {
    pub fn new(cc: &eframe::CreationContext<'_>, choice: BackendChoice) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = egui::Color32::from_rgb(16, 20, 26);
        visuals.window_fill = egui::Color32::from_rgb(22, 27, 34);
        cc.egui_ctx.set_visuals(visuals);

        let curve =
            aero_core::curve_from_percent(&[(45, 35), (60, 45), (75, 60), (85, 80), (90, 100)]);
        let points = curve.points().to_vec();
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
        if let Some(snapshot) = session.initial_sensors.clone() {
            sensor_error = snapshot.error.clone();
            sensors.cpu = snapshot.cpu;
            sensors.gpu = snapshot.gpu;
            sensors.gpu2 = snapshot.gpu2;
            sensors.rpm1 = snapshot.rpm1;
            sensors.rpm2 = snapshot.rpm2;
            sensors.flags = snapshot.flags;
        }
        let app = Self {
            session,
            busy: false,
            report_error: false,
            sensor_error,
            arm_max_test: false,
            tab: Tab::Status,
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
        };
        app
    }

    fn poll_session(&mut self) {
        while let Some(event) = self.session.poll() {
            self.busy = event.busy;
            if let Some(snapshot) = event.sensors {
                self.apply_snapshot(snapshot);
            }
            if let Some(message) = event.message {
                self.report = message;
                self.report_error = event.error;
            }
        }
    }

    fn apply_snapshot(&mut self, snapshot: SensorSnapshot) {
        self.sensors.cpu = snapshot.cpu;
        self.sensors.gpu = snapshot.gpu;
        self.sensors.gpu2 = snapshot.gpu2;
        self.sensors.rpm1 = snapshot.rpm1;
        self.sensors.rpm2 = snapshot.rpm2;
        self.sensors.flags = snapshot.flags;
        self.sensor_error = snapshot.error;
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
                self.session.upload(curve.points().to_vec());
            }
            Err(issues) => {
                self.issues = issues;
                self.report = "Upload stayed disabled: the curve is not valid.".to_string();
                self.report_error = true;
            }
        }
    }
}

impl eframe::App for AeroApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_session();
        ctx.request_repaint_after(std::time::Duration::from_millis(400));
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("aero-control");
                let (label, color) = if self.session.backend_label == "REAL" {
                    if self.session.writes_enabled {
                        ("REAL", egui::Color32::from_rgb(120, 200, 140))
                    } else {
                        ("REAL · read-only", egui::Color32::from_rgb(220, 170, 90))
                    }
                } else {
                    ("MOCK", egui::Color32::from_rgb(140, 170, 210))
                };
                ui.colored_label(color, label);
                ui.selectable_value(&mut self.tab, Tab::Status, "Status");
                ui.selectable_value(&mut self.tab, Tab::Curve, "Fan curve");
            });
            ui.label(
                "Fans are firmware-controlled. Sensor reads are display only and never set a duty.",
            );
            if let Some(banner) = &self.session.banner {
                ui.colored_label(egui::Color32::from_rgb(220, 140, 90), banner);
            }
        });
        egui::TopBottomPanel::bottom("disclaimer").show(ctx, |ui| {
            ui.small("Thermal shutdown still belongs to the CPU, GPU, and embedded controller (PROCHOT and GPU slowdown). aero-control validates curves and restores firmware auto if an upload fails. Quiet, Gaming, and the editor curves are marked provisional where the factory values were not published.");
            if self.report_error {
                ui.colored_label(egui::Color32::from_rgb(220, 120, 110), self.report.as_str());
            } else {
                ui.label(self.report.as_str());
            }
            if let Some(error) = &self.sensor_error {
                ui.colored_label(egui::Color32::from_rgb(220, 120, 110), error);
            }
            ui.small(format!("WMI log: {}", self.session.log_path));
        });
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Status => self.status_ui(ui),
            Tab::Curve => self.curve_ui(ui),
        });
    }
}

impl AeroApp {
    fn status_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            stat(ui, "CPU", &self.sensors.cpu);
            stat(ui, "GPU", &self.sensors.gpu);
            stat(ui, "GPU 2", &self.sensors.gpu2);
            stat(ui, "Fan 1", &self.sensors.rpm1);
            stat(ui, "Fan 2", &self.sensors.rpm2);
        });
        ui.add_space(8.0);
        ui.label(&self.sensors.flags);
        ui.add_space(12.0);
        ui.label("Firmware modes");
        ui.horizontal_wrapped(|ui| {
            let allow = self.writes_allowed();
            if ui.add_enabled(allow, egui::Button::new("Normal")).clicked() {
                self.apply_firmware(FanMode::Normal);
            }
            if ui
                .add_enabled(allow, egui::Button::new("Quiet (Super Quiet)"))
                .clicked()
            {
                self.apply_firmware(FanMode::Quiet);
            }
            if ui.add_enabled(allow, egui::Button::new("Gaming")).clicked() {
                self.apply_firmware(FanMode::Gaming);
            }
            if ui
                .add_enabled(self.writes_allowed(), egui::Button::new("Restore auto"))
                .clicked()
            {
                self.apply_firmware(FanMode::Normal);
            }
            if ui
                .add_enabled(
                    allow,
                    egui::Button::new(if self.arm_max { "Confirm Max" } else { "Max" }),
                )
                .clicked()
            {
                if self.arm_max {
                    self.apply_firmware(FanMode::Max);
                } else {
                    self.arm_max = true;
                    self.report =
                        "Max sets both fans to raw 229 (100%). Click Confirm Max.".to_string();
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Fixed duty");
            ui.add(egui::Slider::new(&mut self.fixed_percent, 0..=100).suffix("%"));
            if ui
                .add_enabled(self.writes_allowed(), egui::Button::new("Apply fixed"))
                .clicked()
            {
                self.apply_firmware(FanMode::Fixed);
            }
            ui.checkbox(&mut self.expert, "Expert floor override");
        });
        ui.label("Fixed mode stays at or above 35% (raw 80) unless expert is on. Closing the window restores firmware auto if the last command was Fixed or Max.");
        if ui
            .add_enabled(
                self.writes_allowed(),
                egui::Button::new(if self.arm_max_test {
                    "Confirm: Max for 10 s then Auto"
                } else {
                    "Test: Max for 10 s then restore Auto"
                }),
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

    fn curve_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            egui::ComboBox::from_label("Load preset")
                .selected_text("Choose")
                .show_ui(ui, |ui| {
                    for preset in presets() {
                        if let PresetKind::Curve(curve) = &preset.kind {
                            let label = if preset.provisional {
                                format!("{} (provisional)", preset.name)
                            } else {
                                preset.name.to_string()
                            };
                            if ui.selectable_label(false, label).clicked() {
                                self.points = curve.points().to_vec();
                                self.recompute();
                                self.report = preset.notes.to_string();
                            }
                        }
                    }
                });
            let valid = self.issues.is_empty();
            if ui
                .add_enabled(valid, egui::Button::new("Upload curve"))
                .clicked()
            {
                self.upload();
            }
        });
        if self.issues.is_empty() {
            ui.colored_label(
                egui::Color32::from_rgb(140, 190, 140),
                "Curve passes validation.",
            );
        } else {
            for issue in &self.issues {
                let where_ = issue
                    .index
                    .map(|index| format!("point {index}: "))
                    .unwrap_or_default();
                ui.colored_label(
                    egui::Color32::from_rgb(220, 120, 110),
                    format!("{where_}{}", issue.message),
                );
            }
        }

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
        let stair_line: PlotPoints = stair.iter().map(|(x, y)| [*x, *y]).collect();
        let markers: PlotPoints = percent_points.iter().map(|(x, y)| [*x, *y]).collect();
        let cpu_x = self
            .sensors
            .cpu
            .trim_end_matches("°C")
            .parse::<f64>()
            .unwrap_or(61.0);

        let response = Plot::new("fan_curve")
            .height(360.0)
            .allow_drag(false)
            .allow_scroll(false)
            .include_x(20.0)
            .include_x(PLOT_X_END)
            .include_y(0.0)
            .include_y(100.0)
            .x_axis_label("Temperature (°C)")
            .y_axis_label("Duty (%)")
            .legend(egui_plot::Legend::default())
            .show(ui, |plot_ui| {
                plot_ui.line(
                    Line::new(stair_line)
                        .name("Stair step (chip, unconfirmed)")
                        .width(2.0_f32),
                );
                plot_ui.points(Points::new(markers).name("Points").radius(6.0_f32));
                plot_ui.vline(VLine::new(cpu_x).name("CPU"));
                plot_ui.pointer_coordinate()
            });
        self.handle_plot(&response);

        ui.label("Drag a point. Double-click the plot to add one (max 15). Right-click a point to remove it (min 2). The stair step holds each duty until the next temperature.");
        self.table(ui);
        ui.separator();
        ui.label("Import a Control Center CustomFanTable (read-only paste). On Windows the registry reader is HKLM\\SOFTWARE\\Gigabyte\\SmartManager, and it never writes that key.");
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
        ui.label("Registry import is compiled in on Windows only. Paste the value here to try the parser.");
        if !self.import_note.is_empty() {
            ui.label(&self.import_note);
        }
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
        egui::Grid::new("points").striped(true).show(ui, |ui| {
            ui.strong("Index");
            ui.strong("°C");
            ui.strong("Duty %");
            ui.strong("Raw");
            ui.end_row();
            let mut changed = false;
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
                    changed = true;
                }
                if ui
                    .add(egui::DragValue::new(&mut percent).range(0..=100))
                    .changed()
                {
                    point.duty_raw = aero_core::percent_to_raw(percent);
                    changed = true;
                }
                if ui
                    .add(egui::DragValue::new(&mut raw).range(0..=229))
                    .changed()
                {
                    point.duty_raw = raw;
                    changed = true;
                }
                ui.end_row();
            }
            if changed {
                // Recomputed after the grid so we are not borrowing points twice.
            }
            let _ = changed;
        });
        self.recompute();
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

fn stat(ui: &mut egui::Ui, title: &str, value: &str) {
    ui.group(|ui| {
        ui.set_min_width(120.0);
        ui.label(title);
        ui.heading(value);
    });
}
