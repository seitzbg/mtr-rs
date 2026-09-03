//! One hop of the path: `struct nethost` (ui/net.c:64-88) plus the reply/send bookkeeping of
//! `save_sequence()` and `net_process_ping()` — mtr 0.96, commit 7b01773. GPL-2.0-only.

use std::net::IpAddr;
use std::time::Instant;

use mtr_proto::MplsLabel;

use crate::MAX_PATH;
use crate::history::{History, Sample};
use crate::stats::RttStats;

/// ICMP error a hop answered with (`nethost.err`, strings from ui/display.c:268-282).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HopError {
    NetworkDown,
    HostDown,
    NoRouteNetwork,
    NoRouteHost,
}

impl HopError {
    pub fn as_str(self) -> &'static str {
        match self {
            HopError::NetworkDown => "network is down",
            HopError::HostDown => "host is down",
            HopError::NoRouteNetwork => "no route to network",
            HopError::NoRouteHost => "no route to host",
        }
    }
}

/// One address seen at a hop (`addrs[]` / `mplss[]`), with per-address counters (new).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HopAddr {
    pub addr: IpAddr,
    pub mpls: Vec<MplsLabel>,
    pub first_seen: Instant,
    pub last_seen: Instant,
    pub count: u32,
    pub last_rtt: u32,
}

/// Everything `net_process_ping()` needs about one reply.
#[derive(Debug, Clone)]
pub struct Reply<'a> {
    pub saved_seq: u32,
    pub from: IpAddr,
    pub rtt_us: u32,
    pub mpls: &'a [MplsLabel],
    pub err: Option<HopError>,
    pub now: Instant,
    /// net.c:280-293: false only when `dueTTL` forbids showing the target at this hop.
    pub overwrite_addr: bool,
    /// `ctl->cache`: remember `seen` for `--cache` skipping.
    pub cache: bool,
}

#[derive(Debug, Clone)]
pub struct Hop {
    /// Latest responder (`addr`); `None` is C's `unspec_addr`.
    pub addr: Option<IpAddr>,
    /// Distinct responders in discovery order, at most `MAX_PATH`.
    pub addrs: Vec<HopAddr>,
    /// Labels of `addr` (`mpls`).
    pub mpls: Vec<MplsLabel>,
    pub err: Option<HopError>,
    /// `sent`: a probe is outstanding and unanswered.
    pub outstanding: bool,
    pub up: bool,
    /// `seen`: last reply time, only tracked with `--cache`.
    pub seen: Option<Instant>,
    pub stats: RttStats,
    pub history: History,
}

impl Hop {
    pub fn new(history_len: usize) -> Self {
        Hop {
            addr: None,
            addrs: Vec::new(),
            mpls: Vec::new(),
            err: None,
            outstanding: false,
            up: false,
            seen: None,
            stats: RttStats::default(),
            history: History::new(history_len),
        }
    }

    pub fn is_unknown(&self) -> bool {
        self.addr.is_none()
    }

    /// `save_sequence()` host part (net.c:146-166). Returns the hop's send counter.
    pub fn record_send(&mut self, now: Instant) -> u32 {
        self.stats.record_send();
        if self.outstanding {
            self.up = false;
        }
        self.outstanding = true;
        let saved_seq = self.stats.xmit as u32;
        self.history.push_sent(saved_seq, now);
        saved_seq
    }

    /// `net_process_ping()` host part (net.c:255-349). Returns true when `from` is new.
    pub fn record_reply(&mut self, r: Reply<'_>) -> bool {
        self.err = r.err;
        let mut new_addr = false;
        if let Some(a) = self.addrs.iter_mut().find(|a| a.addr == r.from) {
            a.count += 1;
            a.last_seen = r.now;
            a.last_rtt = r.rtt_us;
        } else if self.addrs.len() < MAX_PATH {
            self.addrs.push(HopAddr {
                addr: r.from,
                mpls: r.mpls.to_vec(),
                first_seen: r.now,
                last_seen: r.now,
                count: 1,
                last_rtt: r.rtt_us,
            });
            new_addr = true;
        }
        if self.addr != Some(r.from) && r.overwrite_addr {
            self.addr = Some(r.from);
            self.mpls = r.mpls.to_vec();
        }
        self.stats.record_reply(r.rtt_us as i32);
        self.outstanding = false;
        self.up = true;
        if r.cache {
            self.seen = Some(r.now);
        }
        self.history.record(r.saved_seq, Sample::Rtt(r.rtt_us));
        new_addr
    }

    /// Deviation 1: remember a helper timeout in the history only.
    pub fn record_no_reply(&mut self, saved_seq: u32) {
        self.history.record(saved_seq, Sample::Lost);
    }

    /// `net_reset()` for one hop: back to the zeroed template.
    pub fn reset(&mut self) {
        *self = Hop::new(self.history.capacity());
    }

    pub fn loss(&self) -> i32 {
        self.stats.loss()
    }
    pub fn dropped(&self) -> i32 {
        self.stats.dropped()
    }
    pub fn received(&self) -> i32 {
        self.stats.returned
    }
    pub fn transmitted(&self) -> i32 {
        self.stats.xmit
    }
    pub fn last(&self) -> i32 {
        self.stats.last
    }
    pub fn best(&self) -> i32 {
        self.stats.best
    }
    pub fn avg(&self) -> i32 {
        self.stats.avg
    }
    pub fn worst(&self) -> i32 {
        self.stats.worst
    }
    pub fn stdev(&self) -> i32 {
        self.stats.stdev()
    }
    pub fn gmean(&self) -> i32 {
        self.stats.gmean
    }
    pub fn jitter(&self) -> i32 {
        self.stats.jitter
    }
    pub fn javg(&self) -> i32 {
        self.stats.javg
    }
    pub fn jworst(&self) -> i32 {
        self.stats.jworst
    }
    pub fn jinta(&self) -> i32 {
        self.stats.jinta
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Sample;
    use std::net::IpAddr;
    use std::time::Instant;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn reply<'a>(saved_seq: u32, from: &str, rtt_us: u32, now: Instant) -> Reply<'a> {
        Reply {
            saved_seq,
            from: ip(from),
            rtt_us,
            mpls: &[],
            err: None,
            now,
            overwrite_addr: true,
            cache: false,
        }
    }

    #[test]
    fn second_unanswered_send_marks_hop_down() {
        let t = Instant::now();
        let mut h = Hop::new(8);
        assert_eq!(h.record_send(t), 1);
        assert!(h.outstanding && !h.up);
        h.up = true;
        assert_eq!(h.record_send(t), 2);
        assert!(
            !h.up,
            "save_sequence(): a still-outstanding probe marks the hop down"
        );
        assert_eq!(h.stats.xmit, 2);
        assert_eq!(h.history.len(), 2);
    }

    #[test]
    fn reply_sets_address_stats_flags_and_history() {
        let t = Instant::now();
        let mut h = Hop::new(8);
        let seq = h.record_send(t);
        assert!(h.record_reply(reply(seq, "10.0.0.1", 1500, t)));
        assert_eq!(h.addr, Some(ip("10.0.0.1")));
        assert_eq!(h.addrs.len(), 1);
        assert_eq!((h.addrs[0].count, h.addrs[0].last_rtt), (1, 1500));
        assert!(h.up && !h.outstanding);
        assert_eq!((h.stats.returned, h.stats.transit, h.last()), (1, 0, 1500));
        assert_eq!(h.history.latest(), Some(&Sample::Rtt(1500)));
        assert_eq!(h.seen, None);
    }

    #[test]
    fn ecmp_addresses_append_in_order_and_latest_wins() {
        let t = Instant::now();
        let mut h = Hop::new(8);
        let s1 = h.record_send(t);
        assert!(h.record_reply(reply(s1, "10.0.0.1", 100, t)));
        let s2 = h.record_send(t);
        assert!(h.record_reply(reply(s2, "10.0.0.2", 200, t)));
        let s3 = h.record_send(t);
        assert!(!h.record_reply(reply(s3, "10.0.0.1", 300, t)));
        assert_eq!(h.addr, Some(ip("10.0.0.1")));
        assert_eq!(
            h.addrs.iter().map(|a| a.addr).collect::<Vec<_>>(),
            vec![ip("10.0.0.1"), ip("10.0.0.2")]
        );
        assert_eq!(h.addrs[0].count, 2);
    }

    #[test]
    fn overwrite_flag_false_keeps_previous_addr_but_still_records_it() {
        let t = Instant::now();
        let mut h = Hop::new(8);
        let s = h.record_send(t);
        let mut r = reply(s, "192.0.2.10", 100, t);
        r.overwrite_addr = false;
        h.record_reply(r);
        assert_eq!(h.addr, None);
        assert_eq!(h.addrs.len(), 1);
    }

    #[test]
    fn error_replies_set_err_and_normal_replies_clear_it() {
        let t = Instant::now();
        let mut h = Hop::new(8);
        let s = h.record_send(t);
        let mut r = reply(s, "10.0.0.5", 100, t);
        r.err = Some(HopError::NoRouteHost);
        h.record_reply(r);
        assert_eq!(h.err, Some(HopError::NoRouteHost));
        let s = h.record_send(t);
        h.record_reply(reply(s, "10.0.0.5", 100, t));
        assert_eq!(h.err, None);
        assert_eq!(HopError::NoRouteHost.as_str(), "no route to host");
    }

    #[test]
    fn no_reply_only_touches_history() {
        let t = Instant::now();
        let mut h = Hop::new(8);
        let s = h.record_send(t);
        h.record_no_reply(s);
        assert_eq!(h.history.latest(), Some(&Sample::Lost));
        assert_eq!((h.stats.xmit, h.stats.transit, h.stats.returned), (1, 1, 0));
    }

    #[test]
    fn cache_records_seen_and_reset_clears_everything() {
        let t = Instant::now();
        let mut h = Hop::new(8);
        let s = h.record_send(t);
        let mut r = reply(s, "10.0.0.1", 100, t);
        r.cache = true;
        h.record_reply(r);
        assert_eq!(h.seen, Some(t));
        h.reset();
        assert!(h.addr.is_none() && h.addrs.is_empty() && h.history.is_empty());
        assert_eq!(h.stats, RttStats::default());
        assert_eq!(h.history.capacity(), 8);
    }

    #[test]
    fn accessors_expose_stats_in_microseconds() {
        let t = Instant::now();
        let mut h = Hop::new(8);
        let s = h.record_send(t);
        h.record_reply(reply(s, "10.0.0.1", 2500, t));
        assert_eq!(
            (h.transmitted(), h.received(), h.dropped(), h.loss()),
            (1, 1, 0, 0)
        );
        assert_eq!(
            (h.last(), h.best(), h.avg(), h.worst(), h.gmean()),
            (2500, 2500, 2500, 2500, 2500)
        );
        assert_eq!(
            (h.stdev(), h.jitter(), h.javg(), h.jworst(), h.jinta()),
            (0, 0, 0, 0, 0)
        );
    }
}
