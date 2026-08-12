use std::time::{Duration, Instant};

const LONGEST_WAIT: Duration = Duration::from_secs(5 * 60);

pub struct SyncScheduler {
    pending: Option<Instant>,
    debounce: Duration,
    wait: Duration,
}

impl SyncScheduler {
    pub fn new(debounce: Duration) -> Self {
        Self {
            pending: None,
            debounce,
            wait: debounce,
        }
    }

    pub fn request(&mut self, now: Instant) {
        if self.pending.is_none() {
            self.pending = Some(now);
        }
    }

    pub fn retry(&mut self, now: Instant) {
        self.wait = (self.wait * 2).min(LONGEST_WAIT);
        self.pending = Some(now);
    }

    pub fn settled(&mut self) {
        self.wait = self.debounce;
    }

    pub fn waiting_for(&self) -> Duration {
        self.wait
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn take_if_due(&mut self, now: Instant) -> bool {
        match self.pending {
            Some(since) if now.duration_since(since) >= self.wait => {
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

    #[test]
    fn a_flush_the_server_could_not_take_waits_longer_before_the_next_one() {
        let start = Instant::now();
        let mut scheduler = scheduler();

        scheduler.retry(start);
        assert!(!scheduler.take_if_due(start + Duration::from_secs(9)));
        assert!(
            scheduler.take_if_due(start + Duration::from_secs(10)),
            "the first retry waits twice the debounce"
        );

        scheduler.retry(start + Duration::from_secs(10));
        assert!(!scheduler.take_if_due(start + Duration::from_secs(29)));
        assert!(
            scheduler.take_if_due(start + Duration::from_secs(30)),
            "a service that is still down must be left alone for longer"
        );
    }

    #[test]
    fn the_wait_stops_growing_so_a_long_outage_is_still_checked_on() {
        let mut scheduler = scheduler();
        let start = Instant::now();
        for round in 0..30 {
            scheduler.retry(start + Duration::from_secs(round));
        }
        assert_eq!(scheduler.waiting_for(), LONGEST_WAIT);
    }

    #[test]
    fn a_flush_that_landed_puts_the_wait_back_where_it_started() {
        let start = Instant::now();
        let mut scheduler = scheduler();
        scheduler.retry(start);
        scheduler.retry(start);
        scheduler.settled();
        assert_eq!(scheduler.waiting_for(), Duration::from_secs(5));

        scheduler.request(start + Duration::from_secs(60));
        assert!(scheduler.take_if_due(start + Duration::from_secs(65)));
    }

    #[test]
    fn shutting_down_does_not_honour_the_backoff() {
        let mut scheduler = scheduler();
        scheduler.retry(Instant::now());
        assert!(
            scheduler.take_now(),
            "waiting to retry must not lose the changes at exit"
        );
    }
}
