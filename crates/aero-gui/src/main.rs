#![cfg_attr(
    all(windows, not(debug_assertions), not(test)),
    windows_subsystem = "windows"
)]

mod app;
mod cpu_power;
mod desktop_events;
mod device_identity;
mod display_controls;
mod elevate;
mod fan_state;
mod instance;
mod log_file;
mod profile_store;
mod session;
mod startup_task;
mod system_controls;
mod system_power;

use std::env;

use eframe::egui;
use session::BackendChoice;

fn main() -> Result<(), eframe::Error> {
    let args = env::args().skip(1);
    let mut mock = !cfg!(windows);
    let mut background = false;
    let mut startup_action = None;
    for arg in args {
        match arg.as_str() {
            "--mock" => mock = true,
            "--background" => background = true,
            "--enable-startup" if startup_action.is_none() => startup_action = Some(true),
            "--disable-startup" if startup_action.is_none() => startup_action = Some(false),
            "--help" | "-h" => {
                eprintln!(
                    "aero-gui [--mock] [--background | --enable-startup | --disable-startup]\n\n\
                     On Windows the default backend is the laptop's GB_WMIACPI methods.\n\
                     --mock keeps the fake embedded controller.\n\
                     --background starts in the notification area without creating the control window.\n\
                     --enable-startup / --disable-startup change Windows logon startup and exit.\n\
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
    if let Some(enabled) = startup_action {
        if background {
            return Err(launch_error(
                "Startup registration cannot be combined with --background.".into(),
            ));
        }
        let mut task = startup_task::StartupTask::new(mock);
        task.set_enabled(enabled).map_err(launch_error)?;
        println!(
            "Windows background startup {}.",
            if enabled { "enabled" } else { "disabled" }
        );
        return Ok(());
    }
    let Some(instance) = instance::Instance::acquire(background, mock).map_err(launch_error)?
    else {
        return Ok(());
    };
    let choice = if mock {
        BackendChoice::Mock
    } else {
        BackendChoice::Real {
            elevated: elevate::is_elevated(),
        }
    };
    let app = app::AeroApp::new(choice, background, instance);
    run_controller(app, background, run_window)
}

fn run_controller(
    mut app: app::AeroApp,
    mut background: bool,
    mut open_window: impl FnMut(&mut app::AeroApp) -> Result<(), eframe::Error>,
) -> Result<(), eframe::Error> {
    let context = egui::Context::default();
    loop {
        if background {
            loop {
                match app.poll_background(&context) {
                    app::BackgroundAction::Wait => {
                        std::thread::sleep(std::time::Duration::from_millis(100))
                    }
                    app::BackgroundAction::Open => break,
                    app::BackgroundAction::Exit => return Ok(()),
                }
            }
        }
        open_window(&mut app)?;
        if !app.keep_running_after_window_close() {
            return Ok(());
        }
        app.enter_background();
        background = true;
    }
}

fn run_window(app: &mut app::AeroApp) -> Result<(), eframe::Error> {
    let native_options = eframe::NativeOptions {
        // Closing a tray-enabled window returns control to our background
        // loop. Windows does not reliably repaint a hidden native window.
        run_and_return: true,
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
        Box::new(move |cc| {
            app.attach_window(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}

fn launch_error(message: String) -> eframe::Error {
    #[cfg(windows)]
    unsafe {
        use windows::core::{w, PCWSTR};
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let text: Vec<_> = message.encode_utf16().chain(Some(0)).collect();
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            w!("aero-control"),
            MB_OK | MB_ICONERROR,
        );
    }
    eframe::Error::AppCreation(Box::new(std::io::Error::other(message)))
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
