use std::time::{Duration, Instant};

pub const SCROLL_SETTLE: Duration = Duration::from_millis(500);

pub enum Input {
    Dragging,
    Released,
    Scrolled,
}

pub struct LiveDuty {
    applied: u8,
    send_at: Option<Instant>,
}

impl LiveDuty {
    pub fn new(value: u8) -> Self {
        Self {
            applied: value,
            send_at: None,
        }
    }

    pub fn pending(&self, value: u8) -> bool {
        value != self.applied
    }

    pub fn held(&self, value: u8) -> bool {
        self.pending(value) && self.send_at.is_none()
    }

    pub fn input(&mut self, input: Input, now: Instant) {
        self.send_at = match input {
            Input::Dragging => None,
            Input::Released => Some(now),
            Input::Scrolled => Some(now + SCROLL_SETTLE),
        };
    }

    pub fn wait(&self, value: u8, now: Instant) -> Option<Duration> {
        if !self.pending(value) {
            return None;
        }
        self.send_at.map(|at| at.saturating_duration_since(now))
    }

    pub fn applied(&mut self, value: u8) {
        self.applied = value;
        self.send_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_value_is_never_sent() {
        let now = Instant::now();
        let mut duty = LiveDuty::new(50);
        duty.input(Input::Released, now);
        assert_eq!(duty.wait(50, now), None);
    }

    #[test]
    fn dragging_holds_until_release() {
        let now = Instant::now();
        let mut duty = LiveDuty::new(50);
        duty.input(Input::Dragging, now);
        assert!(duty.held(70));
        assert_eq!(duty.wait(70, now + Duration::from_secs(5)), None);
        duty.input(Input::Released, now);
        assert_eq!(duty.wait(70, now), Some(Duration::ZERO));
    }

    #[test]
    fn a_drag_that_returns_to_the_applied_value_sends_nothing() {
        let now = Instant::now();
        let mut duty = LiveDuty::new(50);
        duty.input(Input::Dragging, now);
        duty.input(Input::Released, now);
        assert_eq!(duty.wait(50, now), None);
    }

    #[test]
    fn scrolling_sends_once_after_the_last_step_settles() {
        let start = Instant::now();
        let mut duty = LiveDuty::new(50);
        duty.input(Input::Scrolled, start);
        let last = start + Duration::from_millis(300);
        duty.input(Input::Scrolled, last);
        assert_eq!(
            duty.wait(52, start + SCROLL_SETTLE),
            Some(Duration::from_millis(300))
        );
        assert_eq!(duty.wait(52, last + SCROLL_SETTLE), Some(Duration::ZERO));
        duty.applied(52);
        assert_eq!(duty.wait(52, last + SCROLL_SETTLE), None);
    }
}
