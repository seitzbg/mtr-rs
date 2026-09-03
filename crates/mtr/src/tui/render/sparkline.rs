//! Sparkline scale: `mtr_gen_scale()` / `ms_to_factor()` of ui/curses.c:566-640 (mtr 0.96,
//! commit 7b01773) with 8 buckets (deviation 13). GPL-2.0-only.

use mtr_core::{History, Hop, Sample};

use crate::tui::glyphs::Glyphs;

pub const BUCKETS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scale {
    pub low_us: u32,
    pub high_us: u32,
}

impl Scale {
    /// Path-wide min and max RTT over every remembered sample (curses.c:585-597).
    pub fn from_hops<'a>(hops: impl Iterator<Item = &'a Hop>) -> Scale {
        let mut low = u32::MAX;
        let mut high = 0u32;
        for h in hops {
            for s in h.history.iter() {
                if let Sample::Rtt(us) = s {
                    low = low.min(*us);
                    high = high.max(*us);
                }
            }
        }
        if low == u32::MAX {
            Scale {
                low_us: 0,
                high_us: 0,
            }
        } else {
            Scale {
                low_us: low,
                high_us: high,
            }
        }
    }

    /// `scale[i] = low + range * ((i+1)/N)^2`; the bucket is the first `i` with `rtt <= scale[i]`.
    pub fn bucket(&self, rtt_us: u32) -> usize {
        let range = f64::from(self.high_us.saturating_sub(self.low_us));
        let low = f64::from(self.low_us);
        for i in 0..BUCKETS {
            let f = (i + 1) as f64 / BUCKETS as f64;
            let threshold = low + range * f * f;
            if f64::from(rtt_us) <= threshold {
                return i;
            }
        }
        BUCKETS - 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    /// Bar height bucket (relative, `Scale::bucket`) and the sample's raw RTT in microseconds
    /// (for `Palette::rtt`'s absolute colour — deviation 25).
    Rtt(usize, u32),
    Lost,
    Pending,
}

/// The newest `width` samples, oldest first, left-padded with `Pending` when the history is short.
pub fn cells(history: &History, width: usize, scale: &Scale) -> Vec<Cell> {
    let n = history.len();
    let skip = n.saturating_sub(width);
    let mut out = Vec::with_capacity(width);
    out.resize(width.saturating_sub(n), Cell::Pending);
    out.extend(history.iter().skip(skip).map(|s| match s {
        Sample::Rtt(us) => Cell::Rtt(scale.bucket(*us), *us),
        Sample::Lost => Cell::Lost,
        Sample::Pending { .. } => Cell::Pending,
    }));
    out
}

/// Sparkline cells for a table row: a hop that has never received any reply renders as a blank
/// row (all `Pending`) — its Loss% cell already says 100 %, so a full-height loss slab would just
/// repeat that in a way that reads badly (deviation: never-answered hops are blank, not lossy).
pub fn cells_for_hop(hop: &Hop, width: usize, scale: &Scale) -> Vec<Cell> {
    if hop.received() == 0 {
        vec![Cell::Pending; width]
    } else {
        cells(&hop.history, width, scale)
    }
}

pub fn glyph(cell: &Cell, g: &Glyphs) -> &'static str {
    match cell {
        Cell::Rtt(b, _) => g.bars[(*b).min(BUCKETS - 1)],
        Cell::Lost => g.loss,
        Cell::Pending => g.pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::glyphs::{ASCII, UNICODE};
    use mtr_core::{Hop, Sample};
    use std::time::Instant;

    fn hop_with(samples: &[Sample]) -> Hop {
        let mut h = Hop::new(16);
        let t = Instant::now();
        for s in samples {
            let seq = h.record_send(t);
            match s {
                Sample::Pending { .. } => {}
                other => h.history.record(seq, *other),
            }
        }
        h
    }

    #[test]
    fn scale_spans_min_to_max_over_all_hops_and_ignores_non_rtt() {
        let a = hop_with(&[Sample::Rtt(1000), Sample::Lost, Sample::Rtt(3000)]);
        let b = hop_with(&[
            Sample::Rtt(9000),
            Sample::Pending {
                sent: Instant::now(),
            },
        ]);
        let s = Scale::from_hops([a, b].iter());
        assert_eq!((s.low_us, s.high_us), (1000, 9000));
        assert_eq!(Scale::from_hops(std::iter::empty()).high_us, 0);
    }

    #[test]
    fn buckets_use_squared_factors_like_mtr_gen_scale() {
        // low 0, high 8000: thresholds 8000*((i+1)/8)^2 = 125, 500, 1125, 2000, 3125, 4500, 6125, 8000
        let s = Scale {
            low_us: 0,
            high_us: 8000,
        };
        assert_eq!(s.bucket(0), 0);
        assert_eq!(s.bucket(125), 0);
        assert_eq!(s.bucket(126), 1);
        assert_eq!(s.bucket(2000), 3);
        assert_eq!(s.bucket(2001), 4);
        assert_eq!(s.bucket(8000), 7);
        assert_eq!(s.bucket(99_999), 7, "above the top threshold clamps");
        let flat = Scale {
            low_us: 500,
            high_us: 500,
        };
        assert_eq!(
            flat.bucket(500),
            0,
            "zero range: everything is the lowest bucket"
        );
    }

    #[test]
    fn cells_are_right_aligned_and_newest_last() {
        let h = hop_with(&[Sample::Rtt(0), Sample::Lost, Sample::Rtt(8000)]);
        let s = Scale {
            low_us: 0,
            high_us: 8000,
        };
        let c = cells(&h.history, 5, &s);
        assert_eq!(
            c,
            vec![
                Cell::Pending,
                Cell::Pending,
                Cell::Rtt(0, 0),
                Cell::Lost,
                Cell::Rtt(7, 8000)
            ]
        );
        let c = cells(&h.history, 2, &s);
        assert_eq!(
            c,
            vec![Cell::Lost, Cell::Rtt(7, 8000)],
            "only the newest `width` samples"
        );
        assert_eq!(glyph(&Cell::Rtt(7, 8000), &UNICODE), "█");
        assert_eq!(glyph(&Cell::Lost, &UNICODE), "×");
        assert_eq!(glyph(&Cell::Lost, &ASCII), "x");
        assert_eq!(glyph(&Cell::Pending, &ASCII), " ");
    }

    #[test]
    fn never_answered_hop_renders_as_all_pending() {
        let h = hop_with(&[Sample::Lost, Sample::Lost, Sample::Lost]);
        assert_eq!(h.received(), 0);
        let s = Scale {
            low_us: 0,
            high_us: 0,
        };
        assert_eq!(cells_for_hop(&h, 4, &s), vec![Cell::Pending; 4]);
    }

    #[test]
    fn answered_hop_with_a_drop_still_shows_the_lost_cell() {
        let mut h = hop_with(&[Sample::Rtt(1000), Sample::Lost]);
        // `hop_with` only pokes the history ring, not `stats.returned` (that's `record_reply`'s
        // job); set it directly so `received() > 0` matches "this hop does answer".
        h.stats.returned = 1;
        assert!(h.received() > 0);
        let s = Scale {
            low_us: 1000,
            high_us: 1000,
        };
        assert_eq!(
            cells_for_hop(&h, 2, &s),
            vec![Cell::Rtt(0, 1000), Cell::Lost]
        );
    }
}
