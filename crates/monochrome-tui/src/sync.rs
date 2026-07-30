use std::time::{Duration, Instant};

pub struct SyncScheduler {
    pending: Option<Instant>,
    debounce: Duration,
}

impl SyncScheduler {
    pub fn new(debounce: Duration) -> Self {
        Self {
            pending: None,
            debounce,
        }
    }

    pub fn request(&mut self, now: Instant) {
        if self.pending.is_none() {
            self.pending = Some(now);
        }
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn take_if_due(&mut self, now: Instant) -> bool {
        match self.pending {
            Some(since) if now.duration_since(since) >= self.debounce => {
                self.pending = None;
                true
            }
            _ => false,
        }
    }

    pub fn take_now(&mut self) -> bool {
        self.pending.take().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scheduler() -> SyncScheduler {
        SyncScheduler::new(Duration::from_secs(5))
    }

    #[test]
    fn nothing_is_due_before_anything_is_requested() {
        let mut scheduler = scheduler();
        assert!(!scheduler.is_pending());
        assert!(!scheduler.take_if_due(Instant::now()));
    }

    #[test]
    fn a_request_becomes_due_only_after_the_debounce_window() {
        let start = Instant::now();
        let mut scheduler = scheduler();
        scheduler.request(start);
        assert!(!scheduler.take_if_due(start + Duration::from_secs(4)));
        assert!(scheduler.take_if_due(start + Duration::from_secs(5)));
    }

    #[test]
    fn a_flush_clears_the_request_so_it_does_not_repeat() {
        let start = Instant::now();
        let mut scheduler = scheduler();
        scheduler.request(start);
        assert!(scheduler.take_if_due(start + Duration::from_secs(6)));
        assert!(!scheduler.is_pending());
        assert!(!scheduler.take_if_due(start + Duration::from_secs(60)));
    }

    #[test]
    fn repeated_requests_coalesce_into_one_flush() {
        let start = Instant::now();
        let mut scheduler = scheduler();
        scheduler.request(start);
        scheduler.request(start + Duration::from_secs(1));
        scheduler.request(start + Duration::from_secs(2));
        assert!(scheduler.take_if_due(start + Duration::from_secs(5)));
        assert!(!scheduler.take_if_due(start + Duration::from_secs(5)));
    }

    #[test]
    fn a_later_request_starts_a_fresh_window() {
        let start = Instant::now();
        let mut scheduler = scheduler();
        scheduler.request(start);
        assert!(scheduler.take_if_due(start + Duration::from_secs(5)));
        scheduler.request(start + Duration::from_secs(10));
        assert!(!scheduler.take_if_due(start + Duration::from_secs(12)));
        assert!(scheduler.take_if_due(start + Duration::from_secs(15)));
    }

    #[test]
    fn shutting_down_flushes_whatever_is_still_waiting() {
        let mut scheduler = scheduler();
        scheduler.request(Instant::now());
        assert!(scheduler.take_now());
        assert!(!scheduler.take_now());
    }
}
