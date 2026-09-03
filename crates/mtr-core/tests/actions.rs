// Scenario tests for the interactive actions of ui/select.c:300-372 and ui/curses.c:138-430
// (mtr 0.96, commit 7b01773). GPL-2.0-only.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use mtr_core::fields::{AVAILABLE_OPTIONS, FIELDS};
use mtr_core::{Command, Config, Engine, Event, MAX_PATH, UserAction};
use mtr_proto::{ProbeResult, Protocol, ResponseKind};

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

fn engine(cfg: Config) -> (Engine, Instant) {
    let t0 = Instant::now();
    (Engine::new(cfg, ip("192.0.2.10"), None, t0, 1), t0)
}

fn sends(cmds: &[Command]) -> Vec<(i32, mtr_proto::ProbeParams)> {
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
        .expect("running engine schedules a wake")
}

fn reply(token: i32, from: &str, rtt: u32) -> Event {
    Event::Probe {
        token,
        kind: ResponseKind::Probe {
            result: ProbeResult::TtlExpired,
            addr: ip(from),
            rtt_us: rtt,
            mpls: vec![],
        },
    }
}

fn act(e: &mut Engine, a: UserAction, now: Instant) -> Vec<Command> {
    e.handle(Event::Action(a), now)
}

#[test]
fn available_options_is_exactly_the_field_keys_plus_underscore() {
    let keys: String = FIELDS.iter().map(|f| f.key).collect();
    assert_eq!(AVAILABLE_OPTIONS, format!("{keys}_"));
}

#[test]
fn a_hop_remembers_at_most_max_path_addresses() {
    let (mut e, t0) = engine(Config {
        max_ttl: 1,
        max_ping: u32::MAX,
        ..Config::default()
    });
    let mut now = t0;
    for i in 0..(MAX_PATH + 10) {
        let cmds = e.handle(Event::Tick, now);
        let token = sends(&cmds)[0].0;
        let from = format!("10.{}.{}.1", i / 256, i % 256);
        e.handle(reply(token, &from, 100), now);
        now = wake(&cmds);
    }
    let h = &e.hops()[0];
    assert_eq!(h.addrs.len(), MAX_PATH);
    assert_eq!(
        h.stats.returned as usize,
        MAX_PATH + 10,
        "stats still count every reply"
    );
    assert_eq!(
        h.addr,
        Some(ip("10.0.137.1")),
        "addr follows the latest responder"
    );
}

#[test]
fn toggle_mpls_size_pattern_tos_and_fields_only_change_config() {
    let (mut e, t0) = engine(Config::default());
    e.handle(Event::Tick, t0);
    e.handle(reply(33000, "10.0.0.1", 100), t0);
    act(&mut e, UserAction::ToggleMpls, t0);
    assert!(e.config().mpls);
    act(&mut e, UserAction::SetPacketSize(-200), t0);
    act(&mut e, UserAction::SetBitPattern(-1), t0);
    act(&mut e, UserAction::SetTos(0x10), t0);
    act(&mut e, UserAction::SetFields("DR AGJMXI".into()), t0);
    let c = e.config();
    assert_eq!((c.packet_size, c.bit_pattern, c.tos), (-200, -1, 0x10));
    assert_eq!(c.fields, "DR AGJMXI");
    assert_eq!(e.hops()[0].stats.returned, 1, "no statistics reset");
}

#[test]
fn new_size_and_tos_reach_the_next_batch() {
    let (mut e, t0) = engine(Config {
        max_ttl: 1,
        ..Config::default()
    });
    let t1 = wake(&e.handle(Event::Tick, t0)); // batch 1 done (max_ttl 1)
    act(&mut e, UserAction::SetPacketSize(100), t1);
    act(&mut e, UserAction::SetTos(7), t1);
    let p = sends(&e.handle(Event::Tick, t1))[0].1.clone();
    assert_eq!((p.size, p.tos), (Some(100), Some(7)));
}

#[test]
fn set_max_ttl_clamps_to_first_ttl_and_shortens_the_batch() {
    let (mut e, t0) = engine(Config {
        first_ttl: 3,
        ..Config::default()
    });
    act(&mut e, UserAction::SetMaxTtl(1), t0);
    assert_eq!(
        e.config().max_ttl,
        3,
        "curses.c:325: m < fstTTL is refused → clamp"
    );
    act(&mut e, UserAction::SetMaxTtl(3), t0);
    let cmds = e.handle(Event::Tick, t0);
    assert_eq!(sends(&cmds)[0].1.ttl, Some(3));
    assert_eq!(e.cycles_done(), 1, "ttl 3 == maxTTL ends the batch");
}

#[test]
fn set_interval_changes_the_pacing_of_the_next_tick() {
    let (mut e, t0) = engine(Config {
        max_ttl: 1,
        ..Config::default()
    });
    let t1 = wake(&e.handle(Event::Tick, t0));
    assert_eq!(t1, t0 + Duration::from_secs(1));
    act(&mut e, UserAction::SetInterval(2500), t1);
    let t2 = wake(&e.handle(Event::Tick, t1));
    assert_eq!(t2, t1 + Duration::from_millis(2500));
}

#[test]
fn cycle_protocol_resets_statistics_and_restarts_at_first_ttl() {
    let (mut e, t0) = engine(Config::default());
    let mut now = t0;
    for _ in 0..3 {
        let cmds = e.handle(Event::Tick, now);
        now = wake(&cmds);
    }
    e.handle(reply(33000, "10.0.0.1", 100), now);
    assert_eq!(e.hops()[0].stats.returned, 1);
    act(&mut e, UserAction::CycleProtocol, now);
    assert_eq!(e.config().protocol, Protocol::Udp);
    assert_eq!(e.hops()[0].stats.returned, 0);
    let cmds = e.handle(Event::Tick, now);
    let p = &sends(&cmds)[0].1;
    assert_eq!((p.ttl, p.protocol), (Some(1), Protocol::Udp));
    act(&mut e, UserAction::CycleProtocol, now);
    assert_eq!(e.config().protocol, Protocol::Tcp);
    act(&mut e, UserAction::CycleProtocol, now);
    assert_eq!(e.config().protocol, Protocol::Icmp);
}

#[test]
fn toggles_flip_back_and_resume_without_pause_is_harmless() {
    let (mut e, t0) = engine(Config::default());
    act(&mut e, UserAction::ToggleDns, t0);
    act(&mut e, UserAction::ToggleDns, t0);
    assert!(e.config().dns);
    act(&mut e, UserAction::ToggleAsn, t0);
    act(&mut e, UserAction::ToggleAsn, t0);
    assert!(e.config().ipinfo_fields.is_empty());
    let cmds = act(&mut e, UserAction::Resume, t0);
    assert!(!e.paused());
    assert_eq!(wake(&cmds), t0, "still due immediately");
}

#[test]
fn reset_keeps_config_changes_made_before_it() {
    let (mut e, t0) = engine(Config::default());
    act(&mut e, UserAction::SetTos(9), t0);
    act(&mut e, UserAction::Pause, t0);
    act(&mut e, UserAction::Reset, t0);
    assert_eq!(e.config().tos, 9);
    assert!(e.paused(), "net_reset() does not touch `paused`");
}
