//! First-launch setup and logon controls, with Task Scheduler off the UI thread.

use super::{card, primary_button, secondary_button, section_title, DANGER, MUTED};
use crate::startup_task::StartupStatus;
use eframe::egui::{self, RichText};
use std::sync::mpsc::{self, Receiver, Sender};

enum Request {
    Refresh,
    Set(bool),
    FirstLaunch,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_failed_registration_do_not_mark_setup_complete() {
        let mut tab = StartupTab::new(true);
        let (tx, rx) = mpsc::channel();
        tab.results = Some(rx);
        let status = StartupStatus {
            exists: true,
            enabled: true,
            background: true,
        };
        tx.send((false, Ok(status))).unwrap();
        assert_eq!(tab.poll(), Some(Ok(false)));
        tx.send((true, Err("Task Scheduler rejected registration".into())))
            .unwrap();
        assert!(tab.poll().unwrap().is_err());
        assert!(tab.confirmed.is_none());
        tx.send((true, Ok(status))).unwrap();
        assert_eq!(tab.poll(), Some(Ok(true)));
        assert_eq!(tab.confirmed, Some(status));
    }
}

pub struct StartupTab {
    mock: bool,
    worker: Option<Sender<Request>>,
    results: Option<Receiver<(bool, Result<StartupStatus, String>)>>,
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

    fn start(&mut self, first: Request) {
        let (tx, commands) = mpsc::channel();
        let (results, rx) = mpsc::channel();
        let mock = self.mock;
        std::thread::spawn(move || {
            let mut task = crate::startup_task::StartupTask::new(mock);
            while let Ok(command) = commands.recv() {
                let remember_choice = !matches!(command, Request::Refresh);
                let result = match command {
                    Request::Refresh => task.status(),
                    Request::Set(enabled) => task.set_enabled(enabled).and_then(|_| task.status()),
                    Request::FirstLaunch => task.setup_first_launch(),
                };
                if results.send((remember_choice, result)).is_err() {
                    break;
                }
            }
        });
        self.worker = Some(tx);
        self.results = Some(rx);
        self.request(first);
    }

    pub fn setup_first_launch(&mut self) {
        if self.worker.is_none() {
            self.start(Request::FirstLaunch);
        } else {
            self.request(Request::FirstLaunch);
        }
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

    /// True marks a verified setup/choice; reads and failures never mark it done.
    pub fn poll(&mut self) -> Option<Result<bool, String>> {
        if let Some(rx) = &self.results {
            match rx.try_recv() {
                Ok((remember_choice, Ok(status))) => {
                    self.pending = false;
                    self.confirmed = Some(status);
                    self.error = None;
                    return Some(Ok(remember_choice));
                }
                Ok((_, Err(error))) => {
                    self.pending = false;
                    self.confirmed = None;
                    self.error = Some(error.clone());
                    return Some(Err(error));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = false;
                    self.error = Some("The startup settings worker stopped.".into());
                    self.results = None;
                    return self.error.clone().map(Err);
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        None
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if self.worker.is_none() {
            self.start(Request::Refresh);
        }
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, "Start with Windows");
            ui.label("Restore the remembered fan mode automatically when you sign in, without opening the control window. Aero-control stays in the notification area for sleep/resume; use its tray icon to open it or exit. Windows runs it with administrator privileges. Keep this executable in a permanent location.");
            ui.label("Set up automatically on the first supported launch. Disabling startup is remembered; opening the app again will not turn it back on.");
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
