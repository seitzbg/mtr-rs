//! Per-hop probe history. Replaces the display-only `saved[]` ring of ui/net.c:884-924
//! (`-2` never sent, `-1` pending, `>= 0` RTT) with typed samples. GPL-2.0-only.

use std::collections::VecDeque;
use std::time::Instant;

use mtr_proto::{MplsLabel, ProbeResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sample {
    /// Probe sent, no answer yet (C `saved[] == -1`).
    Pending { sent: Instant },
    /// Round-trip time in microseconds.
    Rtt(u32),
    /// The helper reported `no-reply` (C never records this — deviation 1).
    Lost,
}

/// One probe's full detail, for the TUI Log tab (spec §8 item 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    /// The hop's 1-based send counter (`saved_seq`).
    pub seq: u32,
    pub sent: Instant,
    pub sample: Sample,
    /// What the helper answered; `None` while pending or after `no-reply`.
    pub result: Option<ProbeResult>,
    pub mpls: Vec<MplsLabel>,
}

/// Ring of the most recent probes of one hop, indexed by the hop's 1-based send counter.
#[derive(Debug, Clone)]
pub struct History {
    samples: VecDeque<HistoryEntry>,
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
        self.samples.push_back(HistoryEntry {
            seq: saved_seq,
            sent: now,
            sample: Sample::Pending { sent: now },
            result: None,
            mpls: Vec::new(),
        });
    }

    fn index(&self, saved_seq: u32) -> Option<usize> {
        saved_seq
            .checked_sub(self.base_seq)
            .map(|i| i as usize)
            .filter(|i| *i < self.samples.len())
    }

    /// Overwrite the sample for `saved_seq`; ignored when it has been evicted.
    pub fn record(&mut self, saved_seq: u32, sample: Sample) {
        self.record_outcome(saved_seq, sample, None, &[]);
    }

    /// `record()` plus the helper's result and any MPLS labels.
    pub fn record_outcome(
        &mut self,
        saved_seq: u32,
        sample: Sample,
        result: Option<ProbeResult>,
        mpls: &[MplsLabel],
    ) {
        if let Some(i) = self.index(saved_seq) {
            let e = &mut self.samples[i];
            e.sample = sample;
            e.result = result;
            e.mpls = mpls.to_vec();
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Sample> {
        self.samples.iter().map(|e| &e.sample)
    }

    /// All entries, oldest first, with seq/sent/result/MPLS detail (TUI Log tab).
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &HistoryEntry> {
        self.samples.iter()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn latest(&self) -> Option<&Sample> {
        self.samples.back().map(|e| &e.sample)
    }

    pub fn latest_entry(&self) -> Option<&HistoryEntry> {
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
    fn entries_expose_seq_sent_result_and_mpls() {
        use mtr_proto::{MplsLabel, ProbeResult};
        let t = Instant::now();
        let mut h = History::new(4);
        h.push_sent(1, t);
        h.push_sent(2, t + Duration::from_secs(1));
        let lbl = MplsLabel {
            label: 100,
            tc: 1,
            bottom_of_stack: true,
            ttl: 64,
        };
        h.record_outcome(1, Sample::Rtt(700), Some(ProbeResult::TtlExpired), &[lbl]); // MplsLabel is Copy
        h.record(2, Sample::Lost);
        let e: Vec<&HistoryEntry> = h.entries().collect();
        assert_eq!((e[0].seq, e[0].sent, e[0].sample), (1, t, Sample::Rtt(700)));
        assert_eq!(e[0].result, Some(ProbeResult::TtlExpired));
        assert_eq!(e[0].mpls, vec![lbl]);
        assert_eq!(
            (e[1].seq, e[1].sample, e[1].result),
            (2, Sample::Lost, None)
        );
        assert_eq!(h.latest_entry().unwrap().seq, 2);
        assert_eq!(h.entries().next_back().unwrap().seq, 2);
        assert_eq!(
            h.iter().copied().collect::<Vec<_>>(),
            vec![Sample::Rtt(700), Sample::Lost]
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
