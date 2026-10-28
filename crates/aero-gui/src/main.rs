mod app;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("aero-control")
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([760.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "aero-control",
        native_options,
        Box::new(|cc| Ok(Box::new(app::AeroApp::new(cc)))),
    )
}
