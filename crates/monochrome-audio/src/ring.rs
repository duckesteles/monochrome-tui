use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

pub struct Ring {
    slots: Vec<AtomicU32>,
    mask: usize,
    read: AtomicUsize,
    write: AtomicUsize,
    discards: AtomicUsize,
    discard_to: AtomicUsize,
    honoured: AtomicUsize,
}

impl Ring {
    pub fn with_capacity(requested: usize) -> Self {
        let capacity = requested.next_power_of_two().max(2);
        Self {
            slots: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
            mask: capacity - 1,
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
            discards: AtomicUsize::new(0),
            discard_to: AtomicUsize::new(0),
            honoured: AtomicUsize::new(0),
        }
    }

    pub fn capacity(&self) -> usize {
        self.slots.len() - 1
    }

    pub fn available(&self) -> usize {
        let write = self.write.load(Ordering::Acquire);
        let read = self.read.load(Ordering::Acquire);
        write.wrapping_sub(read)
    }

    pub fn free(&self) -> usize {
        self.capacity() - self.available()
    }

    pub fn is_empty(&self) -> bool {
        self.available() == 0
    }

    pub fn push(&self, samples: &[f32]) -> usize {
        let write = self.write.load(Ordering::Relaxed);
        let count = samples.len().min(self.free());
        for (offset, sample) in samples.iter().take(count).enumerate() {
            let index = write.wrapping_add(offset) & self.mask;
            self.slots[index].store(sample.to_bits(), Ordering::Relaxed);
        }
        self.write
            .store(write.wrapping_add(count), Ordering::Release);
        count
    }

    pub fn pop(&self, out: &mut [f32]) -> usize {
        self.settle();
        let read = self.read.load(Ordering::Relaxed);
        let count = out.len().min(self.available());
        for (offset, slot) in out.iter_mut().take(count).enumerate() {
            let index = read.wrapping_add(offset) & self.mask;
            *slot = f32::from_bits(self.slots[index].load(Ordering::Relaxed));
        }
        self.read.store(read.wrapping_add(count), Ordering::Release);
        count
    }

    pub fn clear(&self) {
        let write = self.write.load(Ordering::Relaxed);
        self.discard_to.store(write, Ordering::Relaxed);
        let asked = self.discards.load(Ordering::Relaxed);
        self.discards
            .store(asked.wrapping_add(1), Ordering::Release);
    }

    pub fn settle(&self) {
        let asked = self.discards.load(Ordering::Acquire);
        if asked == self.honoured.load(Ordering::Relaxed) {
            return;
        }
        self.honoured.store(asked, Ordering::Relaxed);
        let target = self.discard_to.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Relaxed);
        if target.wrapping_sub(read) <= self.capacity() {
            self.read.store(target, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn capacity_rounds_up_to_a_power_of_two_minus_one_slot() {
        assert_eq!(Ring::with_capacity(100).capacity(), 127);
        assert_eq!(Ring::with_capacity(128).capacity(), 127);
    }

    #[test]
    fn samples_come_back_in_order() {
        let ring = Ring::with_capacity(16);
        assert_eq!(ring.push(&[1.0, 2.0, 3.0]), 3);
        let mut out = [0.0; 3];
        assert_eq!(ring.pop(&mut out), 3);
        assert_eq!(out, [1.0, 2.0, 3.0]);
        assert!(ring.is_empty());
    }

    #[test]
    fn pushing_past_capacity_writes_only_what_fits() {
        let ring = Ring::with_capacity(4);
        let written = ring.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(written, ring.capacity());
        assert_eq!(ring.free(), 0);
    }

    #[test]
    fn popping_an_empty_ring_yields_nothing() {
        let ring = Ring::with_capacity(8);
        let mut out = [9.0; 4];
        assert_eq!(ring.pop(&mut out), 0);
        assert_eq!(out, [9.0; 4]);
    }

    #[test]
    fn the_ring_wraps_around_its_end() {
        let ring = Ring::with_capacity(4);
        let mut out = [0.0; 2];
        for round in 0..20 {
            let base = round as f32;
            assert_eq!(ring.push(&[base, base + 0.5]), 2);
            assert_eq!(ring.pop(&mut out), 2);
            assert_eq!(out, [base, base + 0.5]);
        }
    }

    #[test]
    fn clearing_drops_everything_pending() {
        let ring = Ring::with_capacity(8);
        ring.push(&[1.0, 2.0, 3.0]);
        ring.clear();
        let mut out = [0.0; 8];
        assert_eq!(ring.pop(&mut out), 0);
        assert!(ring.is_empty());
        assert_eq!(ring.free(), ring.capacity());
    }

    #[test]
    fn samples_pushed_after_a_clear_are_not_thrown_away_with_the_old_ones() {
        let ring = Ring::with_capacity(16);
        ring.push(&[1.0, 2.0, 3.0]);
        ring.clear();
        assert_eq!(ring.push(&[9.0]), 1);
        let mut out = [0.0; 8];
        assert_eq!(ring.pop(&mut out), 1);
        assert_eq!(out[0], 9.0);
    }

    #[test]
    fn a_clear_that_arrives_after_the_reader_moved_on_does_not_rewind_it() {
        let ring = Ring::with_capacity(8);
        ring.push(&[1.0, 2.0, 3.0]);
        ring.clear();
        ring.push(&[4.0, 5.0]);
        let mut out = [0.0; 4];
        assert_eq!(ring.pop(&mut out), 2);
        ring.settle();
        assert!(
            ring.is_empty(),
            "the reader was pushed back over spent audio"
        );
    }

    #[test]
    fn clearing_while_the_reader_is_running_never_lets_the_ring_overfill() {
        let ring = Arc::new(Ring::with_capacity(64));
        let consumer = Arc::clone(&ring);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let halt = Arc::clone(&stop);

        let reader = std::thread::spawn(move || {
            let mut out = [0.0f32; 32];
            while !halt.load(Ordering::Acquire) {
                consumer.pop(&mut out);
            }
        });

        let block = [0.5f32; 48];
        for round in 0..200_000 {
            let free = ring.free();
            assert!(
                free <= ring.capacity(),
                "free ran past the ring at round {round}"
            );
            assert!(ring.available() <= ring.capacity());
            ring.push(&block);
            if round % 7 == 0 {
                ring.clear();
            }
        }

        stop.store(true, Ordering::Release);
        reader.join().expect("reader finished");
    }

    #[test]
    fn a_producer_and_a_consumer_thread_agree_on_every_sample() {
        let ring = Arc::new(Ring::with_capacity(64));
        let producer = Arc::clone(&ring);
        const TOTAL: usize = 100_000;

        let writer = std::thread::spawn(move || {
            let mut sent = 0usize;
            while sent < TOTAL {
                let value = sent as f32;
                if producer.push(&[value]) == 1 {
                    sent += 1;
                } else {
                    std::hint::spin_loop();
                }
            }
        });

        let mut received = 0usize;
        let mut out = [0.0f32; 1];
        while received < TOTAL {
            if ring.pop(&mut out) == 1 {
                assert_eq!(out[0], received as f32);
                received += 1;
            } else {
                std::hint::spin_loop();
            }
        }
        writer.join().expect("producer finished");
    }
}
