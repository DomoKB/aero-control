mod app;
mod elevate;
mod log_file;
mod session;

use std::env;
use std::process::ExitCode;

use eframe::egui;
use session::BackendChoice;

fn main() -> Result<(), eframe::Error> {
    let mut args = env::args().skip(1);
    let mut mock = !cfg!(windows);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mock" => mock = true,
            "--help" | "-h" => {
                eprintln!(
                    "aero-gui [--mock]\n\n\
                     On Windows the default backend is the laptop's GB_WMIACPI methods.\n\
                     --mock keeps the fake embedded controller.\n\
                     Fan writes require an elevated process. The manifest asks for administrator;\n\
                     if the token is not elevated the window stays read-only."
                );
                return Ok(());
            }
            other => {
                eprintln!("unknown argument {other}");
                return Err(eframe::Error::AppCreation(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    other.to_string(),
                ))));
            }
        }
    }
    if !cfg!(windows) {
        mock = true;
    }
    let choice = if mock {
        BackendChoice::Mock
    } else {
        BackendChoice::Real {
            elevated: elevate::is_elevated(),
        }
    };
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
        Box::new(move |cc| Ok(Box::new(app::AeroApp::new(cc, choice)))),
    )?;
    let _ = ExitCode::SUCCESS;
    Ok(())
}
