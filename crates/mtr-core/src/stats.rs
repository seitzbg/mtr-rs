//! Per-hop RTT statistics with the exact int/double mixing of ui/net.c:296-339 and the
//! accessors at ui/net.c:399-491 (mtr 0.96, commit 7b01773). GPL-2.0-only.
//!
//! Every RTT-like value is microseconds held in an `i32`, as in `struct nethost`.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RttStats {
    /// Probes sent (`xmit`).
    pub xmit: i32,
    /// Replies received (`returned`).
    pub returned: i32,
    /// 0/1 flag: a probe is in flight (`transit`). Cleared by a reply or by `end_transit`.
    pub transit: i32,
    pub last: i32,
    pub best: i32,
    pub worst: i32,
    /// Running mean, truncated to an int after every update (`avg`).
    pub avg: i32,
    pub gmean: i32,
    /// Sum of squared deviations, updated with the already-truncated mean (`ssd`).
    pub ssd: i64,
    pub jitter: i32,
    pub javg: i32,
    pub jworst: i32,
    /// RFC 1889 A.8 estimator, kept ×16 as in C (`jinta`).
    pub jinta: i32,
}

impl RttStats {
    /// `save_sequence()` (net.c:146-166): count the probe and flag it in flight.
    pub fn record_send(&mut self) {
        self.xmit += 1;
        self.transit = 1;
    }

    /// `net_process_ping()` steps 3–10 (net.c:296-343) for a reply of `rtt` microseconds.
    pub fn record_reply(&mut self, rtt: i32) {
        self.jitter = (rtt - self.last).abs();
        self.last = rtt;
        if self.returned < 1 {
            self.best = rtt;
            self.worst = rtt;
            self.gmean = rtt;
            self.avg = 0;
            self.ssd = 0;
            self.jitter = 0;
            self.jworst = 0;
            self.jinta = 0;
        }
        if rtt < self.best {
            self.best = rtt;
        }
        if rtt > self.worst {
            self.worst = rtt;
        }
        if self.jitter > self.jworst {
            self.jworst = self.jitter;
        }
        self.returned += 1;
        let oldavg = self.avg;
        // C: nh->avg += (totusec - oldavg + .0) / nh->returned;   (int += double, truncates)
        self.avg =
            (f64::from(self.avg) + f64::from(rtt - oldavg) / f64::from(self.returned)) as i32;
        // C: nh->ssd += (totusec - oldavg + .0) * (totusec - nh->avg);   (long long += double)
        self.ssd = (self.ssd as f64 + f64::from(rtt - oldavg) * f64::from(rtt - self.avg)) as i64;
        let oldjavg = self.javg;
        // C: nh->javg += (nh->jitter - oldjavg) / nh->returned;   (pure integer division)
        self.javg += (self.jitter - oldjavg) / self.returned;
        // C: nh->jinta += nh->jitter - ((nh->jinta + 8) >> 4);
        self.jinta += self.jitter - ((self.jinta + 8) >> 4);
        if self.returned > 1 {
            let n = f64::from(self.returned);
            self.gmean =
                (f64::from(self.gmean).powf((n - 1.0) / n) * f64::from(rtt).powf(1.0 / n)) as i32;
        }
        self.transit = 0;
    }

    /// `net_loss()` (net.c:399-410): loss percentage × 1000, in-flight probe excluded.
    pub fn loss(&self) -> i32 {
        let denominator = self.xmit - self.transit;
        if denominator == 0 {
            return 0;
        }
        (1000.0 * (100.0 - 100.0 * f64::from(self.returned) / f64::from(denominator))) as i32
    }

    /// `net_drop()` (net.c:413-417).
    pub fn dropped(&self) -> i32 {
        (self.xmit - self.transit) - self.returned
    }

    /// `net_stdev()` (net.c:455-463): sample standard deviation in whole microseconds.
    pub fn stdev(&self) -> i32 {
        if self.returned > 1 {
            (self.ssd as f64 / (f64::from(self.returned) - 1.0)).sqrt() as i32
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rtts: &[i32]) -> RttStats {
        let mut s = RttStats::default();
        for &r in rtts {
            s.record_send();
            s.record_reply(r);
        }
        s
    }

    #[test]
    fn first_reply_initialises_everything() {
        let s = run(&[1000]);
        assert_eq!(
            (
                s.returned, s.last, s.best, s.worst, s.avg, s.gmean, s.ssd, s.jitter, s.javg,
                s.jworst, s.jinta
            ),
            (1, 1000, 1000, 1000, 1000, 1000, 0, 0, 0, 0, 0)
        );
        assert_eq!(s.stdev(), 0);
        assert_eq!((s.xmit, s.transit), (1, 0));
    }

    #[test]
    fn three_replies_match_net_c_arithmetic() {
        // Hand-computed from net.c:296-339 for 1000, 3000, 2000 µs.
        let s = run(&[1000, 3000, 2000]);
        assert_eq!((s.best, s.worst, s.last), (1000, 3000, 2000));
        assert_eq!(s.avg, 2000);
        assert_eq!(s.ssd, 2_000_000);
        assert_eq!(s.stdev(), 1000);
        assert_eq!(s.jitter, 1000);
        assert_eq!(s.jworst, 2000);
        assert_eq!(s.javg, 1000);
        assert_eq!(s.jinta, 2875);
        assert_eq!(s.gmean, 1817);
    }

    #[test]
    fn average_truncates_toward_zero_each_step_like_the_int_field() {
        assert_eq!(run(&[1000, 1001, 1001]).avg, 1000); // real mean 1000.67
        assert_eq!(run(&[3000, 1000, 1000]).avg, 1666); // real mean 1666.67
    }

    #[test]
    fn ssd_uses_the_truncated_average() {
        assert_eq!(run(&[1000, 1001, 1001]).ssd, 2);
        assert_eq!(run(&[1000, 1001, 1001]).stdev(), 1);
    }

    #[test]
    fn javg_uses_integer_division() {
        // jitters: 0, 1000, 1500 -> javg: 0, 0+(1000-0)/2=500, 500+(1500-500)/3=833
        assert_eq!(run(&[1000, 2000, 3500]).javg, 833);
    }

    #[test]
    fn loss_and_drop_exclude_the_in_flight_probe() {
        let mut s = RttStats::default();
        for _ in 0..8 {
            s.record_send();
            s.record_reply(100);
        }
        s.record_send(); // never answered
        s.record_send(); // still in flight
        assert_eq!((s.xmit, s.returned, s.transit), (10, 8, 1));
        assert_eq!(s.loss(), 11111); // 100 - 100*8/9 = 11.111 %
        assert_eq!(s.dropped(), 1);
        s.transit = 0; // net_end_transit()
        assert_eq!(s.loss(), 20000);
        assert_eq!(s.dropped(), 2);
    }

    #[test]
    fn loss_is_zero_before_anything_completes() {
        let mut s = RttStats::default();
        assert_eq!(s.loss(), 0);
        s.record_send();
        assert_eq!((s.loss(), s.dropped()), (0, 0));
    }

    #[test]
    fn rng_is_deterministic_and_bounded() {
        let mut a = crate::rng::Rng::new(7);
        let mut b = crate::rng::Rng::new(7);
        for _ in 0..1000 {
            let x = a.below(256);
            assert_eq!(x, b.below(256));
            assert!(x < 256);
        }
        assert_ne!(
            crate::rng::Rng::new(1).next_u64(),
            crate::rng::Rng::new(2).next_u64()
        );
    }
}
