use std::net::IpAddr;
use std::time::{Duration, Instant};

use mtr_core::{Command, Config, Engine, Event};
use mtr_proto::{ProbeResult, ResponseKind};

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

/// Drive the engine until `Finished`, answering each probe through `answer(ttl)`
/// (`None` = the probe is lost). Time jumps straight to every `NextWake`.
fn run_to_finish(
    e: &mut Engine,
    t0: Instant,
    mut answer: impl FnMut(u8) -> Option<(IpAddr, u32)>,
) -> Instant {
    let mut now = t0;
    loop {
        let cmds = e.handle(Event::Tick, now);
        let mut next = None;
        for c in cmds {
            match c {
                Command::SendProbe { token, params } => {
                    if let Some((from, rtt_us)) = answer(params.ttl.unwrap()) {
                        let result = if from == e.target() {
                            ProbeResult::Reply
                        } else {
                            ProbeResult::TtlExpired
                        };
                        e.handle(
                            Event::Probe {
                                token,
                                kind: ResponseKind::Probe {
                                    result,
                                    addr: from,
                                    rtt_us,
                                    mpls: vec![],
                                },
                            },
                            now,
                        );
                    }
                }
                Command::NextWake(t) => next = Some(t),
                Command::Finished => return now,
                Command::Resolve(_) => {}
            }
        }
        now = next.expect("a running engine always schedules a wake");
    }
}

fn three_hop_path(ttl: u8) -> Option<(IpAddr, u32)> {
    match ttl {
        1 => Some((ip("10.0.0.1"), 500)),
        2 => None,
        _ => Some((ip("192.0.2.10"), 2500)),
    }
}

#[test]
fn linear_discovery_two_cycles() {
    let t0 = Instant::now();
    let cfg = Config {
        interactive: false,
        max_ping: 2,
        ..Config::default()
    };
    let mut e = Engine::new(cfg, ip("192.0.2.10"), Some(ip("192.0.2.1")), t0, 1);
    let end = run_to_finish(&mut e, t0, three_hop_path);

    // Discovery batch probes ttl 1..4 (the target reply lands after ttl 3 was sent), then
    // batch 2 probes ttl 1..3; then one tick of grace detection, then 5 s of grace.
    assert_eq!(e.cycles_done(), 2);
    assert_eq!(e.display_range(), 0..3);
    let h = e.hops();
    assert_eq!(
        (h[0].stats.xmit, h[0].stats.returned, h[0].avg()),
        (2, 2, 500)
    );
    assert_eq!(
        (h[1].stats.xmit, h[1].stats.returned, h[1].loss()),
        (2, 0, 100_000)
    );
    assert_eq!(
        (h[2].stats.xmit, h[2].stats.returned, h[2].avg()),
        (2, 2, 2500)
    );
    assert_eq!(
        h[3].stats.xmit, 1,
        "C probes one hop past the target during discovery"
    );
    assert!(h.iter().all(|h| h.stats.transit == 0), "end_transit ran");
    let elapsed = end.duration_since(t0).as_secs_f64();
    // 0.3 s discovery + 1/3 s gap + 2/3 s second batch + 1/3 s to the grace tick + 5 s grace
    assert!(
        (elapsed - (0.3 + 1.0 / 3.0 + 2.0 / 3.0 + 1.0 / 3.0 + 5.0)).abs() < 1e-3,
        "{elapsed}"
    );
}

#[test]
fn all_lost_path_stops_at_max_unknown_and_reports_loss() {
    let t0 = Instant::now();
    let cfg = Config {
        interactive: false,
        max_ping: 1,
        max_unknown: 3,
        grace_time: 1.0,
        ..Config::default()
    };
    let mut e = Engine::new(cfg, ip("192.0.2.10"), None, t0, 1);
    run_to_finish(&mut e, t0, |_| None);
    // ttl 1..5 probed: when ttl 5 is sent, the 4 unknown hops before it exceed maxUnknown 3
    assert_eq!(e.hops().iter().filter(|h| h.stats.xmit == 1).count(), 5);
    assert_eq!(
        e.display_range(),
        0..0,
        "nothing known => nothing to display (net_max == 0)"
    );
    assert_eq!(e.hops()[0].loss(), 100_000);
}

#[test]
fn cache_skips_a_hop_that_answered_recently() {
    let t0 = Instant::now();
    let cfg = Config {
        max_ttl: 1,
        cache_timeout: Some(Duration::from_secs(60)),
        ..Config::default()
    };
    let mut e = Engine::new(cfg, ip("192.0.2.10"), None, t0, 1);
    let cmds = e.handle(Event::Tick, t0);
    let Command::SendProbe { token, .. } = cmds[0].clone() else {
        panic!("expected a probe")
    };
    e.handle(
        Event::Probe {
            token,
            kind: ResponseKind::Probe {
                result: ProbeResult::Reply,
                addr: ip("192.0.2.10"),
                rtt_us: 100,
                mpls: vec![],
            },
        },
        t0,
    );
    let cmds = e.handle(Event::Tick, t0 + Duration::from_secs(1));
    assert!(
        !cmds.iter().any(|c| matches!(c, Command::SendProbe { .. })),
        "recently seen: skipped"
    );
    assert_eq!(
        e.cycles_done(),
        2,
        "a skipped hop still completes the batch"
    );
    let cmds = e.handle(Event::Tick, t0 + Duration::from_secs(62));
    assert!(
        cmds.iter().any(|c| matches!(c, Command::SendProbe { .. })),
        "cache expired: probed again"
    );
}

#[test]
fn first_ttl_offsets_the_batch_and_the_display_range() {
    let t0 = Instant::now();
    let cfg = Config {
        interactive: false,
        max_ping: 1,
        first_ttl: 3,
        ..Config::default()
    };
    let mut e = Engine::new(cfg, ip("192.0.2.10"), None, t0, 1);
    let seen = std::cell::RefCell::new(Vec::new());
    run_to_finish(&mut e, t0, |ttl| {
        seen.borrow_mut().push(ttl);
        three_hop_path(ttl)
    });
    assert_eq!(
        seen.borrow().first(),
        Some(&3),
        "the first probe uses fstTTL"
    );
    assert_eq!(e.display_range(), 2..3);
    assert_eq!((e.hops()[0].stats.xmit, e.hops()[1].stats.xmit), (0, 0));
    assert_eq!(e.hops()[2].stats.xmit, 1);
}
