//! Per-hop probe history. Replaces the display-only `saved[]` ring of ui/net.c:884-924
//! (`-2` never sent, `-1` pending, `>= 0` RTT) with typed samples. GPL-2.0-only.

use std::collections::VecDeque;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sample {
    /// Probe sent, no answer yet (C `saved[] == -1`).
    Pending { sent: Instant },
    /// Round-trip time in microseconds.
    Rtt(u32),
    /// The helper reported `no-reply` (C never records this — deviation 1).
    Lost,
}

/// Ring of the most recent probes of one hop, indexed by the hop's 1-based send counter.
#[derive(Debug, Clone)]
pub struct History {
    samples: VecDeque<Sample>,
    /// Send counter (`saved_seq`) of `samples[0]`.
    base_seq: u32,
    capacity: usize,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        History {
            samples: VecDeque::new(),
            base_seq: 1,
            capacity: capacity.max(1),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Append a pending sample for send number `saved_seq` (consecutive per hop).
    pub fn push_sent(&mut self, saved_seq: u32, now: Instant) {
        if self.samples.is_empty() {
            self.base_seq = saved_seq;
        }
        debug_assert_eq!(saved_seq, self.base_seq + self.samples.len() as u32);
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
            self.base_seq += 1;
        }
        self.samples.push_back(Sample::Pending { sent: now });
    }

    fn index(&self, saved_seq: u32) -> Option<usize> {
        saved_seq
            .checked_sub(self.base_seq)
            .map(|i| i as usize)
            .filter(|i| *i < self.samples.len())
    }

    /// Overwrite the sample for `saved_seq`; ignored when it has been evicted.
    pub fn record(&mut self, saved_seq: u32, sample: Sample) {
        if let Some(i) = self.index(saved_seq) {
            self.samples[i] = sample;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Sample> {
        self.samples.iter()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn latest(&self) -> Option<&Sample> {
        self.samples.back()
    }

    pub fn clear(&mut self) {
        self.samples.clear();
        self.base_seq = 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn records_by_per_hop_sequence() {
        let t = Instant::now();
        let mut h = History::new(4);
        h.push_sent(1, t);
        h.push_sent(2, t);
        h.push_sent(3, t);
        h.record(2, Sample::Rtt(500));
        h.record(3, Sample::Lost);
        let v: Vec<_> = h.iter().copied().collect();
        assert_eq!(
            v,
            vec![Sample::Pending { sent: t }, Sample::Rtt(500), Sample::Lost]
        );
        assert_eq!(h.latest(), Some(&Sample::Lost));
    }

    #[test]
    fn evicts_oldest_and_keeps_indexing_correct() {
        let t = Instant::now();
        let mut h = History::new(2);
        for seq in 1..=3 {
            h.push_sent(seq, t + Duration::from_secs(seq as u64));
        }
        assert_eq!(h.len(), 2);
        h.record(1, Sample::Rtt(1)); // evicted: ignored
        h.record(3, Sample::Rtt(3));
        let v: Vec<_> = h.iter().copied().collect();
        assert_eq!(
            v,
            vec![
                Sample::Pending {
                    sent: t + Duration::from_secs(2)
                },
                Sample::Rtt(3)
            ]
        );
    }

    #[test]
    fn clear_restarts_numbering() {
        let t = Instant::now();
        let mut h = History::new(4);
        h.push_sent(1, t);
        h.clear();
        assert!(h.is_empty());
        h.push_sent(1, t);
        h.record(1, Sample::Rtt(9));
        assert_eq!(h.latest(), Some(&Sample::Rtt(9)));
    }
}
