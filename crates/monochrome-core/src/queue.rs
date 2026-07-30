use crate::model::Track;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn cycle(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Repeat::Off => "repeat off",
            Repeat::All => "repeat all",
            Repeat::One => "repeat one",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

#[derive(Debug, Default)]
pub struct Queue {
    items: Vec<Track>,
    order: Vec<usize>,
    cursor: Option<usize>,
    shuffle: bool,
    repeat: Repeat,
    seed: u64,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn items(&self) -> &[Track] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn repeat(&self) -> Repeat {
        self.repeat
    }

    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.repeat = repeat;
    }

    pub fn cycle_repeat(&mut self) -> Repeat {
        self.repeat = self.repeat.cycle();
        self.repeat
    }

    pub fn current(&self) -> Option<&Track> {
        self.current_index().and_then(|i| self.items.get(i))
    }

    pub fn current_index(&self) -> Option<usize> {
        self.cursor.and_then(|c| self.order.get(c).copied())
    }

    pub fn replace(&mut self, tracks: Vec<Track>, start_at: usize, seed: u64) {
        self.items = tracks;
        self.seed = seed;
        self.rebuild_order(Some(start_at.min(self.items.len().saturating_sub(1))));
    }

    pub fn append(&mut self, track: Track) {
        let index = self.items.len();
        self.items.push(track);
        if self.shuffle {
            self.order.push(index);
            let tail = self.order.len() - 1;
            let start = self.cursor.map(|c| c + 1).unwrap_or(0);
            if start < tail {
                let mut rng = Rng::new(self.seed ^ index as u64);
                let pick = start + (rng.next() as usize) % (tail - start + 1);
                self.order.swap(pick, tail);
            }
        } else {
            self.order.push(index);
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.order.clear();
        self.cursor = None;
    }

    pub fn remove(&mut self, index: usize) {
        if index >= self.items.len() {
            return;
        }
        let playing = self.current_index();
        self.items.remove(index);
        let next_playing = match playing {
            Some(p) if p == index => None,
            Some(p) if p > index => Some(p - 1),
            other => other,
        };
        self.rebuild_order(next_playing);
    }

    pub fn set_shuffle(&mut self, shuffle: bool) {
        if self.shuffle == shuffle {
            return;
        }
        self.shuffle = shuffle;
        let playing = self.current_index();
        self.rebuild_order(playing);
    }

    pub fn toggle_shuffle(&mut self) -> bool {
        self.set_shuffle(!self.shuffle);
        self.shuffle
    }

    pub fn jump_to(&mut self, index: usize) -> Option<&Track> {
        let position = self.order.iter().position(|&i| i == index)?;
        self.cursor = Some(position);
        self.items.get(index)
    }

    pub fn next(&mut self, manual: bool) -> Option<&Track> {
        if self.order.is_empty() {
            return None;
        }
        if self.repeat == Repeat::One && !manual {
            return self.current();
        }
        let cursor = match self.cursor {
            None => 0,
            Some(c) if c + 1 < self.order.len() => c + 1,
            Some(_) if self.repeat == Repeat::All || manual => 0,
            Some(_) => return None,
        };
        self.cursor = Some(cursor);
        self.current()
    }

    pub fn previous(&mut self) -> Option<&Track> {
        if self.order.is_empty() {
            return None;
        }
        let cursor = match self.cursor {
            None | Some(0) if self.repeat == Repeat::All => self.order.len() - 1,
            None | Some(0) => 0,
            Some(c) => c - 1,
        };
        self.cursor = Some(cursor);
        self.current()
    }

    pub fn has_next(&self) -> bool {
        match self.cursor {
            _ if self.order.is_empty() => false,
            None => true,
            Some(c) => c + 1 < self.order.len() || self.repeat != Repeat::Off,
        }
    }

    fn rebuild_order(&mut self, playing: Option<usize>) {
        self.order = (0..self.items.len()).collect();
        if self.shuffle {
            let mut rng = Rng::new(self.seed);
            let len = self.order.len();
            for i in (1..len).rev() {
                let j = (rng.next() as usize) % (i + 1);
                self.order.swap(i, j);
            }
            if let Some(current) = playing
                && let Some(position) = self.order.iter().position(|&i| i == current)
            {
                self.order.swap(0, position);
            }
        }
        self.cursor = playing.and_then(|p| self.order.iter().position(|&i| i == p));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Quality;

    fn track(id: u64) -> Track {
        Track {
            id,
            title: format!("Track {id}"),
            duration: 200,
            explicit: false,
            artist: None,
            artists: Vec::new(),
            album: None,
            isrc: None,
            track_number: None,
            volume_number: None,
            copyright: None,
            version: None,
            quality: Quality::Lossless,
            replay_gain: None,
            peak: None,
            stream_ready: true,
        }
    }

    fn queue_of(n: u64) -> Queue {
        let mut q = Queue::new();
        q.replace((1..=n).map(track).collect(), 0, 42);
        q
    }

    #[test]
    fn replace_starts_at_requested_index() {
        let mut q = Queue::new();
        q.replace((1..=5).map(track).collect(), 2, 42);
        assert_eq!(q.current().unwrap().id, 3);
    }

    #[test]
    fn next_walks_forward_and_stops_at_end() {
        let mut q = queue_of(3);
        assert_eq!(q.next(false).unwrap().id, 2);
        assert_eq!(q.next(false).unwrap().id, 3);
        assert!(q.next(false).is_none());
    }

    #[test]
    fn repeat_all_wraps_around() {
        let mut q = queue_of(2);
        q.set_repeat(Repeat::All);
        assert_eq!(q.next(false).unwrap().id, 2);
        assert_eq!(q.next(false).unwrap().id, 1);
    }

    #[test]
    fn repeat_one_holds_position_until_manual_skip() {
        let mut q = queue_of(3);
        q.set_repeat(Repeat::One);
        assert_eq!(q.next(false).unwrap().id, 1);
        assert_eq!(q.next(true).unwrap().id, 2);
    }

    #[test]
    fn previous_stops_at_start_without_repeat() {
        let mut q = queue_of(3);
        assert_eq!(q.previous().unwrap().id, 1);
    }

    #[test]
    fn previous_wraps_with_repeat_all() {
        let mut q = queue_of(3);
        q.set_repeat(Repeat::All);
        assert_eq!(q.previous().unwrap().id, 3);
    }

    #[test]
    fn shuffle_keeps_current_track_playing() {
        let mut q = queue_of(20);
        q.next(false);
        let playing = q.current().unwrap().id;
        q.set_shuffle(true);
        assert_eq!(q.current().unwrap().id, playing);
    }

    #[test]
    fn shuffle_visits_every_track_exactly_once() {
        let mut q = queue_of(20);
        q.set_shuffle(true);
        let mut seen = vec![q.current().unwrap().id];
        while let Some(t) = q.next(false) {
            seen.push(t.id);
        }
        seen.sort_unstable();
        assert_eq!(seen, (1..=20).collect::<Vec<_>>());
    }

    #[test]
    fn shuffle_actually_reorders() {
        let mut q = queue_of(40);
        q.set_shuffle(true);
        let mut order = vec![q.current().unwrap().id];
        while let Some(t) = q.next(false) {
            order.push(t.id);
        }
        assert_ne!(order, (1..=40).collect::<Vec<_>>());
    }

    #[test]
    fn disabling_shuffle_restores_natural_order() {
        let mut q = queue_of(10);
        q.set_shuffle(true);
        q.next(false);
        let playing = q.current().unwrap().id;
        q.set_shuffle(false);
        assert_eq!(q.current().unwrap().id, playing);
        assert_eq!(q.next(false).unwrap().id, playing + 1);
    }

    #[test]
    fn removing_the_playing_track_clears_the_cursor() {
        let mut q = queue_of(3);
        q.next(false);
        q.remove(1);
        assert_eq!(q.len(), 2);
        assert!(q.current().is_none());
    }

    #[test]
    fn removing_an_earlier_track_keeps_playback_on_the_same_song() {
        let mut q = queue_of(3);
        q.next(false);
        q.next(false);
        assert_eq!(q.current().unwrap().id, 3);
        q.remove(0);
        assert_eq!(q.current().unwrap().id, 3);
    }

    #[test]
    fn append_adds_to_the_end_in_order_mode() {
        let mut q = queue_of(2);
        q.append(track(9));
        assert_eq!(q.next(false).unwrap().id, 2);
        assert_eq!(q.next(false).unwrap().id, 9);
    }

    #[test]
    fn append_while_shuffled_keeps_every_track_reachable() {
        let mut q = queue_of(5);
        q.set_shuffle(true);
        q.append(track(99));
        let mut seen = vec![q.current().unwrap().id];
        while let Some(t) = q.next(false) {
            seen.push(t.id);
        }
        assert!(seen.contains(&99));
        assert_eq!(seen.len(), 6);
    }

    #[test]
    fn jump_to_selects_the_requested_track() {
        let mut q = queue_of(5);
        assert_eq!(q.jump_to(3).unwrap().id, 4);
        assert_eq!(q.next(false).unwrap().id, 5);
    }

    #[test]
    fn has_next_reports_end_of_queue() {
        let mut q = queue_of(2);
        assert!(q.has_next());
        q.next(false);
        assert!(!q.has_next());
        q.set_repeat(Repeat::All);
        assert!(q.has_next());
    }

    #[test]
    fn empty_queue_is_inert() {
        let mut q = Queue::new();
        assert!(q.next(false).is_none());
        assert!(q.previous().is_none());
        assert!(!q.has_next());
        assert!(q.current().is_none());
    }
}
