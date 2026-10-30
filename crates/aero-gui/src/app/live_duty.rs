use std::time::{Duration, Instant};

pub const DRAG_INTERVAL: Duration = Duration::from_millis(250);

pub struct LiveDuty {
    sent: u8,
    sent_at: Option<Instant>,
}

impl LiveDuty {
    pub fn new(value: u8) -> Self {
        Self {
            sent: value,
            sent_at: None,
        }
    }

    pub fn pending(&self, value: u8) -> bool {
        value != self.sent
    }

    pub fn due(&self, value: u8, dragging: bool, now: Instant) -> bool {
        self.pending(value)
            && (!dragging
                || self
                    .sent_at
                    .is_none_or(|at| now.duration_since(at) >= DRAG_INTERVAL))
    }

    pub fn mark_sent(&mut self, value: u8, now: Instant) {
        self.sent = value;
        self.sent_at = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_value_is_never_due() {
        let duty = LiveDuty::new(50);
        assert!(!duty.due(50, false, Instant::now()));
        assert!(!duty.due(50, true, Instant::now()));
    }

    #[test]
    fn release_sends_the_final_value_at_once() {
        let now = Instant::now();
        let mut duty = LiveDuty::new(50);
        duty.mark_sent(60, now);
        assert!(duty.due(61, false, now));
    }

    #[test]
    fn dragging_coalesces_to_one_write_per_interval() {
        let start = Instant::now();
        let mut duty = LiveDuty::new(50);
        assert!(duty.due(55, true, start));
        duty.mark_sent(55, start);
        assert!(!duty.due(58, true, start + Duration::from_millis(100)));
        assert!(!duty.due(62, true, start + Duration::from_millis(249)));
        assert!(duty.due(62, true, start + DRAG_INTERVAL));
    }
}
