//! Explicit logon startup, with Task Scheduler operations off the UI thread.

use super::{card, primary_button, secondary_button, section_title, DANGER, MUTED};
use eframe::egui::{self, RichText};
use std::sync::mpsc::{self, Receiver, Sender};

enum Request {
    Refresh,
    Set(bool),
}

pub struct StartupTab {
    mock: bool,
    worker: Option<Sender<Request>>,
    results: Option<Receiver<Result<bool, String>>>,
    confirmed: Option<bool>,
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
                    Request::Refresh => task.enabled(),
                    Request::Set(enabled) => task.set_enabled(enabled).and_then(|_| task.enabled()),
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
                Ok(Ok(enabled)) => {
                    self.pending = false;
                    self.confirmed = Some(enabled);
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
            ui.label("Optional: launch this copy of aero-control when you log on. Windows runs it with administrator privileges so saved profiles can be applied.");
            if self.mock {
                ui.label(RichText::new("Mock mode: no Windows task is created or changed.").color(MUTED));
            }
            ui.horizontal_wrapped(|ui| {
                if self.pending { ui.spinner(); ui.label("Checking startup settings…"); }
                if let Some(enabled) = self.confirmed {
                    ui.label(if enabled { "Enabled" } else { "Disabled" });
                    if ui.add_enabled(!self.pending, primary_button(if enabled { "Disable logon startup" } else { "Enable logon startup" })).clicked() {
                        self.request(Request::Set(!enabled));
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
