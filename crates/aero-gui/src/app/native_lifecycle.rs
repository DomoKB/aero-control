//! Native renderer/notification-area regression tests. Hardware and settings
//! stay mocked. Each scenario gets a child process so a lost Exit is a bounded
//! failure, rather than hanging the test runner or touching the real controller.

use super::*;
use crate::desktop_events::{DesktopEvent, DesktopEvents};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use winit::platform::windows::EventLoopBuilderExtWindows;

const TEST_NAME: &str = "app::native_lifecycle::native_tray_exit_stops_the_process";
const SCENARIO: &str = "AERO_TEST_TRAY_SCENARIO";

#[test]
#[ignore = "Requires an interactive Windows desktop and native renderer; uses mock hardware"]
fn native_tray_exit_stops_the_process() {
    if let Ok(scenario) = std::env::var(SCENARIO) {
        run_scenario(&scenario);
        return;
    }
    for scenario in ["background", "visible", "hidden", "reopen", "no_tray"] {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", TEST_NAME, "--nocapture"])
            .env(SCENARIO, scenario)
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW (test runner console only)
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "native tray scenario {scenario}: {status}"
                );
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill(); // Only this test's isolated mock child.
                let _ = child.wait();
                panic!("native tray scenario {scenario}: process did not exit");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn run_scenario(scenario: &str) {
    let instance = crate::instance::Instance::acquire(true, true)
        .unwrap()
        .unwrap();
    let background = scenario != "no_tray";
    let mut app = AeroApp::new(BackendChoice::Mock, background, instance);
    app.desktop = DesktopEvents::new(false);
    let events = app.desktop.test_sender();
    let (opened_tx, opened_rx) = mpsc::channel::<egui::Context>();
    let scenario = scenario.to_owned();
    let driver = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        if scenario != "background" {
            if scenario != "no_tray" {
                events.send(DesktopEvent::Show).unwrap();
            }
            let ctx = opened_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            std::thread::sleep(Duration::from_millis(600));
            if scenario == "no_tray" {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            if scenario == "hidden" || scenario == "reopen" {
                // Same close request delivered by the title-bar X.
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                std::thread::sleep(Duration::from_millis(700));
                if scenario == "reopen" {
                    events.send(DesktopEvent::FanMode(FanMode::Gaming)).unwrap();
                    std::thread::sleep(Duration::from_millis(600));
                    events.send(DesktopEvent::Show).unwrap();
                    let ctx = opened_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    // A second round trip catches renderer/context reuse bugs.
                    std::thread::sleep(Duration::from_millis(300));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    std::thread::sleep(Duration::from_millis(500));
                    events.send(DesktopEvent::Show).unwrap();
                    opened_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                }
            }
        }
        events.send(DesktopEvent::Exit).unwrap();
    });
    let mut windows_opened = 0;
    crate::run_controller(app, background, |app| {
        windows_opened += 1;
        if windows_opened > 1 {
            assert_eq!(app.fan_apply.active_mode, Some(FanMode::Gaming));
            assert_eq!(
                app.profiles.fan_restore_profile().unwrap().fan_mode,
                FanMode::Gaming
            );
        }
        eframe::run_native(
            "aero-control lifecycle test (mock)",
            eframe::NativeOptions {
                viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 650.0]),
                event_loop_builder: Some(Box::new(|builder| {
                    builder.with_any_thread(true);
                })),
                ..Default::default()
            },
            Box::new(|cc| {
                app.attach_window(&cc.egui_ctx);
                opened_tx.send(cc.egui_ctx.clone()).unwrap();
                Ok(Box::new(app))
            }),
        )
    })
    .unwrap();
    driver.join().unwrap();
    // The guard is released only after all controller workers have shut down.
    // Probe from a different thread: a Windows mutex is recursive on its
    // owning thread, which would make a same-thread check a false positive.
    assert!(
        std::thread::spawn(|| crate::instance::Instance::acquire(true, true)
            .unwrap()
            .is_some())
        .join()
        .unwrap()
    );
}
