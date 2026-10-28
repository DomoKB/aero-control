use std::thread;
use std::time::Duration;

/// Gap Control Center leaves between WMI calls. The transport itself does not
/// sleep; the probe and the apply engine pace, so unit tests can use [`NoPacer`].
pub const WMI_PACE: Duration = Duration::from_millis(500);

pub trait Pacer {
    fn pause(&mut self);
}

#[derive(Debug, Default)]
pub struct NoPacer;

impl Pacer for NoPacer {
    fn pause(&mut self) {}
}

#[derive(Debug, Clone)]
pub struct FixedPacer {
    pub interval: Duration,
}

impl FixedPacer {
    pub fn standard() -> Self {
        Self { interval: WMI_PACE }
    }
}

impl Pacer for FixedPacer {
    fn pause(&mut self) {
        thread::sleep(self.interval);
    }
}

#[derive(Debug, Default)]
pub struct CountPacer {
    pub pauses: usize,
}

impl Pacer for CountPacer {
    fn pause(&mut self) {
        self.pauses += 1;
    }
}
