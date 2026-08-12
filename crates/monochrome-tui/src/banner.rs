use std::time::{Duration, Instant};

pub struct Banner {
    showing: Option<String>,
    since: Instant,
    lifetime: Duration,
}

impl Banner {
    pub fn new(lifetime: Duration, now: Instant) -> Self {
        Self {
            showing: None,
            since: now,
            lifetime,
        }
    }

    pub fn watch(&mut self, message: Option<&str>, now: Instant) {
        if self.showing.as_deref() == message {
            return;
        }
        self.showing = message.map(str::to_owned);
        self.since = now;
    }

    pub fn is_stale(&self, now: Instant) -> bool {
        self.showing.is_some() && now.duration_since(self.since) > self.lifetime
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIFETIME: Duration = Duration::from_secs(5);

    fn banner(start: Instant) -> Banner {
        Banner::new(LIFETIME, start)
    }

    #[test]
    fn an_empty_banner_never_goes_stale() {
        let start = Instant::now();
        let banner = banner(start);
        assert!(!banner.is_stale(start + Duration::from_secs(600)));
    }

    #[test]
    fn a_message_gets_its_full_time_no_matter_how_late_it_arrived() {
        let start = Instant::now();
        let mut banner = banner(start);

        let arrived = start + Duration::from_secs(14);
        banner.watch(Some("the search failed"), arrived);
        assert!(
            !banner.is_stale(arrived + Duration::from_secs(4)),
            "a message that took a while to arrive must still be readable"
        );
        assert!(banner.is_stale(arrived + Duration::from_secs(6)));
    }

    #[test]
    fn a_new_message_starts_its_own_clock() {
        let start = Instant::now();
        let mut banner = banner(start);
        banner.watch(Some("first"), start);
        let later = start + Duration::from_secs(4);
        banner.watch(Some("second"), later);
        assert!(!banner.is_stale(later + Duration::from_secs(4)));
        assert!(banner.is_stale(later + Duration::from_secs(6)));
    }

    #[test]
    fn the_same_message_left_on_screen_is_not_given_extra_time() {
        let start = Instant::now();
        let mut banner = banner(start);
        banner.watch(Some("muted"), start);
        for second in 1..=6 {
            banner.watch(Some("muted"), start + Duration::from_secs(second));
        }
        assert!(banner.is_stale(start + Duration::from_secs(6)));
    }

    #[test]
    fn clearing_the_banner_stops_the_clock() {
        let start = Instant::now();
        let mut banner = banner(start);
        banner.watch(Some("muted"), start);
        banner.watch(None, start + Duration::from_secs(1));
        assert!(!banner.is_stale(start + Duration::from_secs(600)));
    }
}
