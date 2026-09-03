//! The probing state machine. Ported from ui/net.c (net_send_batch, net_send_query,
//! new_sequence, save_sequence, net_process_ping, net_reset, net_max/net_min) and the tick
//! block of ui/select.c:104-372 — mtr 0.96, commit 7b01773. GPL-2.0-only.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use mtr_proto::{ProbeParams, ProbeResult, Protocol, ResponseKind};

use crate::config::Config;
use crate::hop::{Hop, HopError, Reply};
use crate::rng::Rng;
use crate::{DEFAULT_NUMHOSTS, HISTORY_LEN, MAX_HOST, MAX_SEQUENCE, MIN_PACKET, MIN_SEQUENCE};

/// Interactive actions (the curses keys of ui/select.c:300-360).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserAction {
    Pause,
    Resume,
    Reset,
    ToggleDns,
    ToggleAsn,
    ToggleMpls,
    /// New probe interval in milliseconds.
    SetInterval(u32),
    SetPacketSize(i32),
    SetBitPattern(i32),
    SetTos(u8),
    SetFirstTtl(u8),
    SetMaxTtl(u8),
    CycleProtocol,
    SetFields(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Time advanced to `now`: send or expire whatever is due.
    Tick,
    /// A line from the helper. Probe replies and `no-reply` are consumed; other kinds ignored.
    Probe {
        token: i32,
        kind: ResponseKind,
    },
    Action(UserAction),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    SendProbe {
        token: i32,
        params: ProbeParams,
    },
    /// A never-before-seen address answered: look up its name / ASN.
    Resolve(IpAddr),
    /// Deliver the next `Event::Tick` at this instant (earlier if other events arrive).
    NextWake(Instant),
    /// Cycles and grace period are over; statistics are final.
    Finished,
}

/// `struct sequence` (net.c:91-96) without its dead `time` field.
#[derive(Debug, Clone, Copy, Default)]
struct SeqSlot {
    hop: u8,
    transit: bool,
    saved_seq: u32,
}

pub struct Engine {
    cfg: Config,
    target: IpAddr,
    local: Option<IpAddr>,
    hops: Vec<Hop>,
    seqs: Vec<SeqSlot>,
    /// `new_sequence()` counter; never reset (static in C).
    next_sequence: u32,
    /// Hop index probed by the next `net_send_batch()` call.
    batch_at: usize,
    /// Pacing divisor: probes in the last completed batch.
    numhosts: u32,
    /// Completed batches (`NumPing`).
    num_ping: u32,
    /// Packet size / bit pattern chosen at the start of the current batch.
    packet_size: i32,
    bit_pattern: i32,
    next_send: Option<Instant>,
    grace_start: Option<Instant>,
    finished: bool,
    paused: bool,
    started: Instant,
    rng: Rng,
}

impl Engine {
    pub fn new(
        cfg: Config,
        target: IpAddr,
        local: Option<IpAddr>,
        now: Instant,
        seed: u64,
    ) -> Self {
        let first = usize::from(cfg.first_ttl.max(1)) - 1;
        Engine {
            hops: (0..MAX_HOST).map(|_| Hop::new(HISTORY_LEN)).collect(),
            seqs: vec![SeqSlot::default(); MAX_SEQUENCE as usize],
            next_sequence: MIN_SEQUENCE,
            batch_at: first,
            numhosts: DEFAULT_NUMHOSTS,
            num_ping: 0,
            packet_size: cfg.packet_size,
            bit_pattern: cfg.bit_pattern,
            next_send: None,
            grace_start: None,
            finished: false,
            paused: false,
            started: now,
            rng: Rng::new(seed),
            cfg,
            target,
            local,
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn target(&self) -> IpAddr {
        self.target
    }

    pub fn local(&self) -> Option<IpAddr> {
        self.local
    }

    pub fn hops(&self) -> &[Hop] {
        &self.hops
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Completed batches (`NumPing`).
    pub fn cycles_done(&self) -> u32 {
        self.num_ping
    }

    pub fn started(&self) -> Instant {
        self.started
    }

    /// `net_min()`: index of the first displayed hop (`fstTTL - 1`).
    pub fn min_hop(&self) -> usize {
        usize::from(self.cfg.first_ttl.max(1)) - 1
    }

    /// `net_max()` (net.c:494-519): one past the last displayed hop.
    pub fn max_hop(&self) -> usize {
        let max_ttl = usize::from(self.cfg.max_ttl).min(MAX_HOST);
        let mut max = 0;
        for at in 0..max_ttl {
            let h = &self.hops[at];
            if h.addr == Some(self.target) || h.err.is_some() {
                return at + 1;
            }
            if h.addr.is_some() {
                max = at + 2;
            }
        }
        max.min(max_ttl)
    }

    /// The hops a report or the TUI shows: `net_min() .. net_max()`.
    pub fn display_range(&self) -> std::ops::Range<usize> {
        let lo = self.min_hop();
        lo..self.max_hop().max(lo)
    }

    pub fn handle(&mut self, ev: Event, now: Instant) -> Vec<Command> {
        let mut out = Vec::new();
        match ev {
            Event::Tick => self.tick(now, &mut out),
            Event::Probe { token, kind } => self.on_probe(token, kind, now, &mut out),
            Event::Action(a) => self.on_action(a),
        }
        if let Some(t) = self.next_wake(now) {
            out.push(Command::NextWake(t));
        }
        out
    }

    fn next_wake(&self, now: Instant) -> Option<Instant> {
        // select.c:180-191: while paused the whole tick block — grace expiry included — is skipped.
        if self.finished || self.paused {
            return None;
        }
        if let Some(g) = self.grace_start {
            return Some(g + secs(self.cfg.grace_time));
        }
        Some(self.next_send.unwrap_or(now))
    }

    /// One pass of the select_loop() tick block (select.c:202-227).
    fn tick(&mut self, now: Instant, out: &mut Vec<Command>) {
        if self.finished || self.paused {
            return;
        }
        if let Some(g) = self.grace_start {
            // Deviation 2: `>=` instead of C's strict `>`.
            if now.duration_since(g) >= secs(self.cfg.grace_time) {
                self.finish(out);
            }
            return;
        }
        if let Some(t) = self.next_send {
            if now < t {
                return;
            }
        }
        if self.num_ping >= self.cfg.max_ping && (!self.cfg.interactive || self.cfg.force_max_ping)
        {
            self.grace_start = Some(now);
            return;
        }
        if self.send_batch(now, out) {
            self.num_ping += 1;
        }
        // calc_deltatime() (net.c:138-143): WaitTime / numhosts.
        let dt = secs(self.cfg.interval / f64::from(self.numhosts.max(1)));
        self.next_send = Some(now + dt);
    }

    fn finish(&mut self, out: &mut Vec<Command>) {
        self.finished = true;
        self.end_transit();
        out.push(Command::Finished);
    }

    /// `net_end_transit()` (net.c:564-572): a final in-flight probe counts as dropped.
    pub fn end_transit(&mut self) {
        for h in &mut self.hops {
            h.stats.transit = 0;
        }
    }

    /// `net_send_batch()` (net.c:574-652): probe one hop; true when this call ended a batch.
    fn send_batch(&mut self, now: Instant, out: &mut Vec<Command>) -> bool {
        let first = self.min_hop();
        let at = self.batch_at;
        if at < usize::from(self.cfg.first_ttl) {
            // Batch start: choose packet size and bit pattern (net.c:583-608).
            self.packet_size = if self.cfg.packet_size < 0 {
                let m = -self.cfg.packet_size;
                if m <= MIN_PACKET {
                    MIN_PACKET
                } else {
                    MIN_PACKET + self.rng.below((m - MIN_PACKET) as u32) as i32
                }
            } else {
                self.cfg.packet_size
            };
            self.bit_pattern = if self.cfg.bit_pattern < 0 {
                -(256 + self.rng.below(256) as i32)
            } else {
                self.cfg.bit_pattern
            };
        }
        // --cache: skip a hop that answered recently (net.c:610-614).
        let cache_skip = match (self.cfg.cache_timeout, self.hops[at].seen) {
            (Some(t), Some(seen)) => self.hops[at].up && now.duration_since(seen) <= t,
            _ => false,
        };
        if !cache_skip {
            self.send_query(at, now, out);
        }
        // Scan the hops before this one (net.c:616-633).
        let due = usize::from(self.cfg.due_ttl);
        let mut n_unknown = 0u32;
        let mut restart = false;
        for i in first..at {
            if self.hops[i].addr.is_none() {
                n_unknown += 1;
            }
            if self.hops[i].addr == Some(self.target) && due <= i + 1 {
                restart = true;
                self.numhosts = (i - first + 1) as u32;
                break;
            }
        }
        // Batch-end decision (net.c:635-643).
        let due_ok = due <= at + 1;
        if (n_unknown > self.cfg.max_unknown || self.hops[at].addr == Some(self.target)) && due_ok
            || at + 1 >= usize::from(self.cfg.max_ttl)
        {
            restart = true;
            self.numhosts = (at - first + 1) as u32;
        }
        if restart {
            self.batch_at = first;
            return true;
        }
        self.batch_at += 1;
        false
    }

    /// `net_send_query()` + `new_sequence()` + `save_sequence()` (net.c:146-197).
    fn send_query(&mut self, at: usize, now: Instant, out: &mut Vec<Command>) {
        let seq = self.next_sequence;
        self.next_sequence += 1;
        if self.next_sequence >= MAX_SEQUENCE {
            self.next_sequence = MIN_SEQUENCE;
        }
        let saved_seq = self.hops[at].record_send(now);
        self.seqs[seq as usize] = SeqSlot {
            hop: at as u8,
            transit: true,
            saved_seq,
        };
        let mut p = ProbeParams::new(self.target);
        p.local_ip = self.local;
        p.protocol = self.cfg.protocol;
        p.size = Some(self.packet_size.unsigned_abs().min(65535) as u16); // abs(packetsize)
        p.bit_pattern = Some((self.bit_pattern & 0xff) as u8);
        p.tos = Some(self.cfg.tos);
        p.ttl = Some((at + 1) as u8);
        p.timeout_s = Some(self.cfg.probe_timeout.as_secs() as u32);
        if self.cfg.remote_port != 0 {
            p.port = Some(self.cfg.remote_port);
        }
        if self.cfg.local_port != 0 {
            p.local_port = Some(self.cfg.local_port);
        }
        if self.cfg.mark != 0 {
            p.mark = Some(self.cfg.mark);
        }
        p.local_device = self.cfg.interface.clone();
        out.push(Command::SendProbe {
            token: seq as i32,
            params: p,
        });
    }

    /// `net_process_ping()` (net.c:228-350) plus deviation 1 for `no-reply`.
    fn on_probe(&mut self, token: i32, kind: ResponseKind, now: Instant, out: &mut Vec<Command>) {
        let Ok(seq) = usize::try_from(token) else {
            return;
        };
        if seq >= MAX_SEQUENCE as usize {
            return;
        }
        match kind {
            ResponseKind::Probe {
                result,
                addr,
                rtt_us,
                mpls,
            } => {
                // mark_sequence_complete() (net.c:206-219): duplicate/stale tokens are dropped.
                let slot = self.seqs[seq];
                if !slot.transit {
                    return;
                }
                self.seqs[seq].transit = false;
                let at = usize::from(slot.hop);
                let err = match result {
                    ProbeResult::Reply | ProbeResult::TtlExpired => None,
                    ProbeResult::NoRouteHost => Some(HopError::NoRouteHost),
                };
                // net.c:280-293: with dueTTL the target may not be shown at an earlier hop.
                let due = usize::from(self.cfg.due_ttl);
                let overwrite_addr = if due > 0 && addr == self.target {
                    due <= at + 1
                } else {
                    true
                };
                let new_addr = self.hops[at].record_reply(Reply {
                    saved_seq: slot.saved_seq,
                    from: addr,
                    rtt_us,
                    mpls: &mpls,
                    err,
                    now,
                    overwrite_addr,
                    cache: self.cfg.cache_timeout.is_some(),
                });
                if new_addr {
                    out.push(Command::Resolve(addr));
                }
            }
            ResponseKind::NoReply => {
                let slot = self.seqs[seq];
                if slot.transit {
                    self.hops[usize::from(slot.hop)].record_no_reply(slot.saved_seq);
                }
                // `transit` deliberately stays set: C never learns about timeouts (cmdpipe.c:768-782).
            }
            _ => {} // handshake and error replies belong to the client
        }
    }

    fn on_action(&mut self, a: UserAction) {
        match a {
            UserAction::Pause => self.paused = true,
            UserAction::Resume => self.paused = false,
            UserAction::Reset => self.reset(),
            UserAction::ToggleDns => self.cfg.dns = !self.cfg.dns,
            UserAction::ToggleMpls => self.cfg.mpls = !self.cfg.mpls,
            UserAction::ToggleAsn => {
                if self.cfg.ipinfo_fields.is_empty() {
                    self.cfg.ipinfo_fields = vec![0];
                } else {
                    self.cfg.ipinfo_fields.clear();
                }
            }
            UserAction::SetInterval(ms) => self.cfg.interval = f64::from(ms.max(1)) / 1000.0,
            UserAction::SetPacketSize(n) => self.cfg.packet_size = n,
            UserAction::SetBitPattern(n) => self.cfg.bit_pattern = n,
            UserAction::SetTos(t) => self.cfg.tos = t,
            UserAction::SetFirstTtl(t) => {
                // curses.c:302-317: the `f` key only changes fstTTL; statistics are kept.
                self.cfg.first_ttl = t.clamp(1, self.cfg.max_ttl.max(1));
                self.batch_at = self.batch_at.max(self.min_hop());
            }
            UserAction::SetMaxTtl(t) => self.cfg.max_ttl = t.max(self.cfg.first_ttl.max(1)),
            UserAction::CycleProtocol => {
                self.cfg.protocol = match self.cfg.protocol {
                    Protocol::Icmp => Protocol::Udp,
                    Protocol::Udp => Protocol::Tcp,
                    Protocol::Tcp | Protocol::Sctp => Protocol::Icmp,
                };
                self.reset();
            }
            UserAction::SetFields(s) => self.cfg.fields = s,
        }
    }

    /// `net_reset()` (net.c:843-866): hops and sequence flags are cleared; the token counter is not.
    pub fn reset(&mut self) {
        for h in &mut self.hops {
            h.reset();
        }
        for s in &mut self.seqs {
            s.transit = false;
        }
        self.batch_at = self.min_hop();
        self.numhosts = DEFAULT_NUMHOSTS;
    }
}

/// `Duration::from_secs_f64` without its panics: non-finite or non-positive seconds become zero.
fn secs(seconds: f64) -> Duration {
    if seconds.is_finite() && seconds > 0.0 {
        Duration::from_secs_f64(seconds)
    } else {
        Duration::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn cfg() -> Config {
        Config {
            interactive: false,
            ..Config::default()
        }
    }

    fn engine(c: Config) -> (Engine, Instant) {
        let t0 = Instant::now();
        (
            Engine::new(
                c,
                "192.0.2.10".parse().unwrap(),
                Some("192.0.2.1".parse().unwrap()),
                t0,
                1,
            ),
            t0,
        )
    }

    fn sends(cmds: &[Command]) -> Vec<(i32, ProbeParams)> {
        cmds.iter()
            .filter_map(|c| match c {
                Command::SendProbe { token, params } => Some((*token, params.clone())),
                _ => None,
            })
            .collect()
    }

    fn wake(cmds: &[Command]) -> Instant {
        cmds.iter()
            .find_map(|c| match c {
                Command::NextWake(t) => Some(*t),
                _ => None,
            })
            .expect("a running engine always schedules a wake")
    }

    #[test]
    fn first_tick_sends_ttl_one_with_c_field_values() {
        let (mut e, t0) = engine(cfg());
        let cmds = e.handle(Event::Tick, t0);
        let s = sends(&cmds);
        assert_eq!(s.len(), 1);
        let (token, p) = &s[0];
        assert_eq!(*token, 33000);
        assert_eq!(p.target, "192.0.2.10".parse::<IpAddr>().unwrap());
        assert_eq!(p.local_ip, Some("192.0.2.1".parse().unwrap()));
        assert_eq!(
            (p.ttl, p.size, p.bit_pattern, p.tos, p.timeout_s),
            (Some(1), Some(64), Some(0), Some(0), Some(10))
        );
        assert_eq!(
            (p.port, p.local_port, p.mark, &p.local_device),
            (None, None, None, &None)
        );
        assert_eq!(wake(&cmds), t0 + Duration::from_millis(100)); // 1.0 s / numhosts(10)
        assert_eq!(e.hops()[0].stats.xmit, 1);
    }

    #[test]
    fn early_tick_sends_nothing_and_keeps_the_wake() {
        let (mut e, t0) = engine(cfg());
        e.handle(Event::Tick, t0);
        let cmds = e.handle(Event::Tick, t0 + Duration::from_millis(50));
        assert!(sends(&cmds).is_empty());
        assert_eq!(wake(&cmds), t0 + Duration::from_millis(100));
    }

    #[test]
    fn optional_fields_are_forwarded_when_set() {
        let c = Config {
            remote_port: 443,
            local_port: 40000,
            mark: 9,
            interface: Some("eth0".into()),
            tos: 16,
            probe_timeout: Duration::from_secs(3),
            protocol: mtr_proto::Protocol::Tcp,
            ..cfg()
        };
        let (mut e, t0) = engine(c);
        let p = sends(&e.handle(Event::Tick, t0))[0].1.clone();
        assert_eq!(
            (p.port, p.local_port, p.mark, p.tos, p.timeout_s),
            (Some(443), Some(40000), Some(9), Some(16), Some(3))
        );
        assert_eq!(p.local_device.as_deref(), Some("eth0"));
        assert_eq!(p.protocol, mtr_proto::Protocol::Tcp);
    }

    #[test]
    fn batch_ends_at_max_ttl_and_numhosts_shrinks() {
        let (mut e, t0) = engine(Config {
            max_ttl: 3,
            ..cfg()
        });
        let mut now = t0;
        for ttl in 1..=3u8 {
            let cmds = e.handle(Event::Tick, now);
            assert_eq!(sends(&cmds)[0].1.ttl, Some(ttl));
            now = wake(&cmds);
        }
        assert_eq!(e.cycles_done(), 1);
        // The third probe completed a 3-probe batch, so the gap became 1/3 s.
        let gap = now
            .duration_since(t0 + Duration::from_millis(200))
            .as_secs_f64();
        assert!((gap - 1.0 / 3.0).abs() < 1e-6, "{gap}");
        let cmds = e.handle(Event::Tick, now);
        assert_eq!(sends(&cmds)[0].1.ttl, Some(1));
    }

    #[test]
    fn max_unknown_ends_the_batch() {
        let (mut e, t0) = engine(Config {
            max_unknown: 2,
            ..cfg()
        });
        let mut now = t0;
        let mut ttls = Vec::new();
        for _ in 0..5 {
            let cmds = e.handle(Event::Tick, now);
            ttls.push(sends(&cmds)[0].1.ttl.unwrap());
            now = wake(&cmds);
        }
        // hops 1..3 unknown (3 > maxUnknown 2) when hop 4 is probed -> batch restarts
        assert_eq!(ttls, vec![1, 2, 3, 4, 1]);
        assert_eq!(e.cycles_done(), 1);
    }

    #[test]
    fn tokens_increment_and_wrap_at_65536() {
        let (mut e, t0) = engine(Config {
            max_ttl: 1,
            max_ping: u32::MAX,
            ..cfg()
        });
        let mut now = t0;
        let mut last = 0;
        for i in 0..32540 {
            let cmds = e.handle(Event::Tick, now);
            let s = sends(&cmds);
            assert_eq!(s.len(), 1);
            last = s[0].0;
            if i == 0 {
                assert_eq!(last, 33000);
            }
            now = wake(&cmds);
        }
        assert_eq!(last, 33003); // 32536 distinct tokens, then 33000, 33001, 33002, 33003
    }

    #[test]
    fn random_size_and_pattern_are_chosen_once_per_batch() {
        let (mut e, t0) = engine(Config {
            packet_size: -100,
            bit_pattern: -1,
            max_ttl: 2,
            ..cfg()
        });
        let a = sends(&e.handle(Event::Tick, t0))[0].1.clone();
        let b = sends(&e.handle(Event::Tick, t0 + Duration::from_millis(100)))[0]
            .1
            .clone();
        assert!((28..100).contains(&a.size.unwrap()), "{:?}", a.size);
        assert_eq!(a.size, b.size);
        assert_eq!(a.bit_pattern, b.bit_pattern);
        assert!(a.bit_pattern.is_some());
    }

    #[test]
    fn paused_engine_sends_nothing_and_schedules_no_wake() {
        let (mut e, t0) = engine(Config::default()); // interactive
        e.handle(Event::Tick, t0);
        let cmds = e.handle(Event::Action(UserAction::Pause), t0);
        assert!(!cmds.iter().any(|c| matches!(c, Command::NextWake(_))));
        assert!(e.paused());
        let cmds = e.handle(Event::Tick, t0 + Duration::from_millis(100));
        assert!(sends(&cmds).is_empty());
        let cmds = e.handle(
            Event::Action(UserAction::Resume),
            t0 + Duration::from_millis(150),
        );
        assert_eq!(wake(&cmds), t0 + Duration::from_millis(100)); // overdue: driver ticks at once
        let cmds = e.handle(Event::Tick, t0 + Duration::from_millis(200));
        assert_eq!(sends(&cmds).len(), 1);
    }

    #[test]
    fn pause_freezes_the_grace_countdown_like_select_c() {
        let (mut e, t0) = engine(Config {
            max_ttl: 1,
            max_ping: 1,
            force_max_ping: true,
            grace_time: 0.5,
            ..Config::default()
        });
        let t1 = wake(&e.handle(Event::Tick, t0)); // batch 1 complete
        e.handle(Event::Tick, t1); // grace period starts
        let cmds = e.handle(Event::Action(UserAction::Pause), t1);
        assert!(!cmds.iter().any(|c| matches!(c, Command::NextWake(_))));
        let cmds = e.handle(Event::Tick, t1 + Duration::from_secs(5));
        assert!(
            !cmds.contains(&Command::Finished),
            "paused: the grace clock is not consulted"
        );
        let cmds = e.handle(
            Event::Action(UserAction::Resume),
            t1 + Duration::from_secs(5),
        );
        assert_eq!(wake(&cmds), t1 + Duration::from_millis(500));
        let cmds = e.handle(Event::Tick, t1 + Duration::from_secs(5));
        assert!(cmds.contains(&Command::Finished));
    }

    #[test]
    fn non_positive_timing_values_do_not_panic() {
        let (mut e, t0) = engine(Config {
            interval: -1.0,
            grace_time: f64::NAN,
            max_ttl: 1,
            max_ping: 1,
            ..cfg()
        });
        let t1 = wake(&e.handle(Event::Tick, t0));
        assert_eq!(t1, t0, "a non-positive interval means 'send immediately'");
        e.handle(Event::Tick, t1); // grace starts
        let cmds = e.handle(Event::Tick, t1);
        assert!(
            cmds.contains(&Command::Finished),
            "a NaN grace time collapses to zero"
        );
    }

    fn probe(token: i32, from: &str, rtt: u32) -> Event {
        Event::Probe {
            token,
            kind: ResponseKind::Probe {
                result: ProbeResult::TtlExpired,
                addr: from.parse().unwrap(),
                rtt_us: rtt,
                mpls: vec![],
            },
        }
    }

    fn resolves(cmds: &[Command]) -> Vec<IpAddr> {
        cmds.iter()
            .filter_map(|c| match c {
                Command::Resolve(ip) => Some(*ip),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn reply_updates_hop_and_requests_resolution() {
        let (mut e, t0) = engine(cfg());
        e.handle(Event::Tick, t0);
        let cmds = e.handle(
            probe(33000, "10.0.0.1", 1500),
            t0 + Duration::from_millis(2),
        );
        assert_eq!(resolves(&cmds), vec!["10.0.0.1".parse::<IpAddr>().unwrap()]);
        let h = &e.hops()[0];
        assert_eq!(h.addr, Some("10.0.0.1".parse().unwrap()));
        assert_eq!((h.stats.returned, h.stats.transit, h.last()), (1, 0, 1500));
        assert!(h.up && !h.outstanding);
        assert_eq!(h.history.latest(), Some(&crate::Sample::Rtt(1500)));
    }

    #[test]
    fn duplicate_late_and_bogus_tokens_are_ignored() {
        let (mut e, t0) = engine(cfg());
        e.handle(Event::Tick, t0);
        e.handle(probe(33000, "10.0.0.1", 1500), t0);
        let cmds = e.handle(probe(33000, "10.0.0.1", 1500), t0);
        assert!(resolves(&cmds).is_empty());
        assert_eq!(e.hops()[0].stats.returned, 1);
        for bogus in [99, -1, 70_000, i32::MAX] {
            e.handle(probe(bogus, "10.0.0.2", 1), t0);
        }
        assert_eq!(e.hops()[0].stats.returned, 1);
        assert_eq!(e.hops()[0].addrs.len(), 1);
    }

    #[test]
    fn no_reply_marks_history_lost_but_keeps_stats() {
        let (mut e, t0) = engine(cfg());
        e.handle(Event::Tick, t0);
        e.handle(
            Event::Probe {
                token: 33000,
                kind: ResponseKind::NoReply,
            },
            t0 + Duration::from_secs(10),
        );
        let h = &e.hops()[0];
        assert_eq!(h.history.latest(), Some(&crate::Sample::Lost));
        assert_eq!((h.stats.xmit, h.stats.transit, h.stats.returned), (1, 1, 0));
        // a reply after no-reply is still accepted, as in C (transit was never cleared)
        e.handle(probe(33000, "10.0.0.1", 5), t0 + Duration::from_secs(11));
        assert_eq!(e.hops()[0].stats.returned, 1);
    }

    #[test]
    fn ecmp_addresses_accumulate_and_resolve_once_each() {
        let (mut e, t0) = engine(Config {
            max_ttl: 1,
            max_ping: 100,
            ..cfg()
        });
        let mut now = t0;
        let mut all_resolves = Vec::new();
        for from in ["10.0.0.1", "10.0.0.2", "10.0.0.1"] {
            let cmds = e.handle(Event::Tick, now);
            let token = sends(&cmds)[0].0;
            all_resolves.extend(resolves(&e.handle(probe(token, from, 100), now)));
            now = wake(&cmds);
        }
        let ips: Vec<IpAddr> = ["10.0.0.1", "10.0.0.2"]
            .iter()
            .map(|s| s.parse().unwrap())
            .collect();
        assert_eq!(all_resolves, ips);
        assert_eq!(
            e.hops()[0].addrs.iter().map(|a| a.addr).collect::<Vec<_>>(),
            ips
        );
        assert_eq!(e.hops()[0].addr, Some(ips[0]));
    }

    #[test]
    fn due_ttl_hides_an_early_target() {
        let (mut e, t0) = engine(Config {
            due_ttl: 3,
            ..cfg()
        });
        e.handle(Event::Tick, t0);
        e.handle(probe(33000, "192.0.2.10", 100), t0);
        assert_eq!(e.hops()[0].addr, None);
        assert_eq!(e.hops()[0].addrs.len(), 1);
    }

    #[test]
    fn no_route_host_sets_err_and_caps_display_range() {
        let (mut e, t0) = engine(cfg());
        e.handle(Event::Tick, t0);
        e.handle(
            Event::Probe {
                token: 33000,
                kind: ResponseKind::Probe {
                    result: ProbeResult::NoRouteHost,
                    addr: "10.0.0.5".parse().unwrap(),
                    rtt_us: 7,
                    mpls: vec![],
                },
            },
            t0,
        );
        assert_eq!(e.hops()[0].err, Some(crate::HopError::NoRouteHost));
        assert_eq!(e.display_range(), 0..1);
    }

    #[test]
    fn display_range_follows_net_max() {
        let (mut e, t0) = engine(cfg());
        assert_eq!(e.display_range(), 0..0);
        let mut now = t0;
        let mut tokens = Vec::new();
        for _ in 0..3 {
            let cmds = e.handle(Event::Tick, now);
            tokens.push(sends(&cmds)[0].0);
            now = wake(&cmds);
        }
        e.handle(probe(tokens[0], "10.0.0.1", 100), now);
        assert_eq!(e.display_range(), 0..2); // known hop + the pending one after it
        e.handle(probe(tokens[2], "192.0.2.10", 100), now);
        assert_eq!(e.display_range(), 0..3);
    }

    #[test]
    fn cycles_then_grace_then_finished_with_end_transit() {
        let (mut e, t0) = engine(Config {
            max_ttl: 1,
            max_ping: 1,
            grace_time: 0.5,
            ..cfg()
        });
        let cmds = e.handle(Event::Tick, t0);
        assert_eq!(sends(&cmds).len(), 1);
        assert_eq!(e.cycles_done(), 1);
        let t1 = wake(&cmds);
        assert_eq!(t1, t0 + Duration::from_secs(1)); // numhosts is 1 now
        let cmds = e.handle(Event::Tick, t1);
        assert!(sends(&cmds).is_empty());
        assert_eq!(wake(&cmds), t1 + Duration::from_millis(500));
        let cmds = e.handle(Event::Tick, t1 + Duration::from_millis(500));
        assert!(cmds.contains(&Command::Finished));
        assert!(!cmds.iter().any(|c| matches!(c, Command::NextWake(_))));
        assert!(e.is_finished());
        assert_eq!(e.hops()[0].stats.transit, 0);
        assert_eq!(e.hops()[0].loss(), 100_000);
        assert!(
            e.handle(Event::Tick, t1 + Duration::from_secs(9))
                .is_empty()
        );
    }

    #[test]
    fn interactive_without_c_never_finishes() {
        let (mut e, t0) = engine(Config {
            max_ttl: 1,
            ..Config::default()
        });
        let mut now = t0;
        for _ in 0..30 {
            let cmds = e.handle(Event::Tick, now);
            assert!(!cmds.contains(&Command::Finished));
            now = wake(&cmds);
        }
        assert_eq!(e.cycles_done(), 30);
        let (mut e, t0) = engine(Config {
            max_ttl: 1,
            max_ping: 2,
            force_max_ping: true,
            ..Config::default()
        });
        let mut now = t0;
        let mut finished = false;
        for _ in 0..10 {
            let cmds = e.handle(Event::Tick, now);
            if cmds.contains(&Command::Finished) {
                finished = true;
                break;
            }
            now = wake(&cmds);
        }
        assert!(finished, "-c makes interactive mode finish");
    }

    #[test]
    fn reset_clears_hops_but_keeps_token_counter() {
        let (mut e, t0) = engine(cfg());
        e.handle(Event::Tick, t0);
        e.handle(probe(33000, "10.0.0.1", 100), t0);
        e.handle(Event::Action(UserAction::Reset), t0);
        assert!(e.hops()[0].addr.is_none() && e.hops()[0].stats.returned == 0);
        let cmds = e.handle(Event::Tick, t0 + Duration::from_millis(100));
        assert_eq!(sends(&cmds)[0].0, 33001);
        assert_eq!(sends(&cmds)[0].1.ttl, Some(1)); // batch_at went back to fstTTL-1
    }

    #[test]
    fn set_first_ttl_keeps_statistics_like_the_curses_f_key() {
        let (mut e, t0) = engine(cfg());
        e.handle(Event::Tick, t0);
        e.handle(probe(33000, "10.0.0.1", 100), t0);
        e.handle(Event::Action(UserAction::SetFirstTtl(2)), t0);
        assert_eq!(e.hops()[0].stats.returned, 1, "no reset");
        assert_eq!(e.min_hop(), 1);
        let cmds = e.handle(Event::Tick, t0 + Duration::from_millis(100));
        assert_eq!(
            sends(&cmds)[0].1.ttl,
            Some(2),
            "batch_at realigned to the new first hop"
        );
    }

    #[test]
    fn cycle_protocol_and_toggles_update_config() {
        let (mut e, t0) = engine(cfg());
        e.handle(Event::Action(UserAction::CycleProtocol), t0);
        assert_eq!(e.config().protocol, Protocol::Udp);
        e.handle(Event::Action(UserAction::ToggleAsn), t0);
        assert_eq!(e.config().ipinfo_fields, vec![0]);
        e.handle(Event::Action(UserAction::ToggleDns), t0);
        assert!(!e.config().dns);
        e.handle(Event::Action(UserAction::SetInterval(250)), t0);
        assert_eq!(e.config().interval, 0.25);
    }
}
