//! The probing state machine. Ported from ui/net.c (net_send_batch, net_send_query,
//! new_sequence, save_sequence, net_process_ping, net_reset, net_max/net_min) and the tick
//! block of ui/select.c:104-372 — mtr 0.96, commit 7b01773. GPL-2.0-only.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use mtr_proto::{ProbeParams, ResponseKind};

use crate::config::Config;
use crate::hop::Hop;
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
#[allow(dead_code)]
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

    fn on_probe(
        &mut self,
        _token: i32,
        _kind: ResponseKind,
        _now: Instant,
        _out: &mut Vec<Command>,
    ) {
        // Task 10
    }

    fn on_action(&mut self, a: UserAction) {
        match a {
            UserAction::Pause => self.paused = true,
            UserAction::Resume => self.paused = false,
            _ => {} // Task 10
        }
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
}
