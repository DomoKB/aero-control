//! Explicit logon startup, with Task Scheduler operations off the UI thread.

use super::{card, primary_button, secondary_button, section_title, DANGER, MUTED};
use crate::startup_task::StartupStatus;
use eframe::egui::{self, RichText};
use std::sync::mpsc::{self, Receiver, Sender};

enum Request {
    Refresh,
    Set(bool),
}

pub struct StartupTab {
    mock: bool,
    worker: Option<Sender<Request>>,
    results: Option<Receiver<Result<StartupStatus, String>>>,
    confirmed: Option<StartupStatus>,
    pending: bool,
    error: Option<String>,
}

impl StartupTab {
    pub fn new(mock: bool) -> Self {
        Self {
            mock,
            worker: None,
            results: None,
            confirmed: None,
            pending: false,
            error: None,
        }
    }

    fn start(&mut self) {
        let (tx, commands) = mpsc::channel();
        let (results, rx) = mpsc::channel();
        let mock = self.mock;
        std::thread::spawn(move || {
            let mut task = crate::startup_task::StartupTask::new(mock);
            while let Ok(command) = commands.recv() {
                let result = match command {
                    Request::Refresh => task.status(),
                    Request::Set(enabled) => task.set_enabled(enabled).and_then(|_| task.status()),
                };
                if results.send(result).is_err() {
                    break;
                }
            }
        });
        self.worker = Some(tx);
        self.results = Some(rx);
        self.request(Request::Refresh);
    }

    fn request(&mut self, request: Request) {
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.send(request).is_ok())
        {
            self.pending = true;
            self.error = None;
        } else {
            self.error = Some("The startup settings worker is unavailable.".into());
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if self.worker.is_none() {
            self.start();
        }
        if let Some(rx) = &self.results {
            match rx.try_recv() {
                Ok(Ok(status)) => {
                    self.pending = false;
                    self.confirmed = Some(status);
                    self.error = None;
                }
                Ok(Err(error)) => {
                    self.pending = false;
                    self.confirmed = None;
                    self.error = Some(error);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = false;
                    self.error = Some("The startup settings worker stopped.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, "Start with Windows");
            ui.label("Restore the remembered fan mode automatically when you sign in, without opening the control window. Aero-control stays in the notification area for sleep/resume; use its tray icon to open it or exit. Windows runs it with administrator privileges. Keep this executable in a permanent location.");
            if self.mock {
                ui.label(RichText::new("Mock mode: no Windows task is created or changed.").color(MUTED));
            }
            ui.horizontal_wrapped(|ui| {
                if self.pending { ui.spinner(); ui.label("Checking startup settings…"); }
                if let Some(status) = self.confirmed {
                    ui.label(if !status.enabled { "Disabled" } else if status.background { "Enabled — background" } else { "Enabled — opens the window" });
                    if ui.add_enabled(!self.pending, primary_button(if status.enabled { "Disable logon startup" } else { "Enable logon startup" })).clicked() {
                        self.request(Request::Set(!status.enabled));
                    }
                    if status.enabled && !status.background && ui.add_enabled(!self.pending, secondary_button("Use background startup")).clicked() {
                        self.request(Request::Set(true));
                    }
                }
                if ui.add_enabled(!self.pending, secondary_button("Refresh status")).clicked() {
                    self.request(Request::Refresh);
                }
            });
            if let Some(error) = &self.error { ui.colored_label(DANGER, error); }
        });
    }
}
