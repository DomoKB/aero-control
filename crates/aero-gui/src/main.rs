mod app;
mod elevate;
mod log_file;
mod session;
mod system_power;

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
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([880.0, 620.0])
            .with_icon(app_icon()),
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

fn app_icon() -> egui::IconData {
    const N: u32 = 64;
    let mut rgba = vec![0u8; (N * N * 4) as usize];
    let cx = 31.5_f32;
    let cy = 31.5_f32;
    for y in 0..N {
        for x in 0..N {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            let i = ((y * N + x) * 4) as usize;
            if d > 30.0 {
                continue;
            }
            rgba[i] = 16;
            rgba[i + 1] = 22;
            rgba[i + 2] = 28;
            rgba[i + 3] = 255;
            if (d - 22.0).abs() < 2.4 || d < 5.0 {
                rgba[i] = 64;
                rgba[i + 1] = 196;
                rgba[i + 2] = 168;
            }
            let sector = dy.atan2(dx).rem_euclid(std::f32::consts::TAU / 3.0);
            let blade =
                (sector - std::f32::consts::TAU / 6.0).abs() < 0.32 && (7.0..20.0).contains(&d);
            if blade {
                rgba[i] = 236;
                rgba[i + 1] = 242;
                rgba[i + 2] = 240;
            }
        }
    }
    egui::IconData {
        rgba,
        width: N,
        height: N,
    }
}
