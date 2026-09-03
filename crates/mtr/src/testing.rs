//! Test fixtures shared by the renderer unit tests and the `tui_snapshots` integration test.
//! `#[doc(hidden)]`: compiled into the library so integration tests can use it, not public API.
//! GPL-2.0-only.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use mtr_core::{Command, Config, Engine, Event};
use mtr_proto::{MplsLabel, ProbeResult, ResponseKind};
use ratatui::buffer::Buffer;

use crate::names::NameCache;
use crate::tui::glyphs::{Glyphs, UNICODE};
use crate::tui::palette::{Depth, Palette};
use crate::tui::render::View;
use crate::tui::state::UiState;

pub fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

/// What the fake "network" answers for one probe.
pub enum Answer {
    NoReply,
    Reply {
        addr: IpAddr,
        rtt_us: u32,
        mpls: Vec<MplsLabel>,
    },
}

/// Drive `cfg`'s engine to `Command::Finished`, answering every probe with `answer(ttl, cycle)`
/// (`cycle` counts from 1 per TTL). Returns the engine and the instant it finished.
pub fn drive(cfg: Config, mut answer: impl FnMut(u8, u32) -> Answer) -> (Engine, Instant) {
    let t0 = Instant::now();
    let mut e = Engine::new(cfg, ip("192.0.2.10"), Some(ip("192.0.2.1")), t0, 1);
    let mut now = t0;
    let mut seen: HashMap<u8, u32> = HashMap::new();
    loop {
        let cmds = e.handle(Event::Tick, now);
        let mut next = None;
        let mut done = false;
        for c in cmds {
            match c {
                Command::SendProbe { token, params } => {
                    let ttl = params.ttl.expect("probes carry a TTL");
                    let cycle = {
                        let n = seen.entry(ttl).or_insert(0);
                        *n += 1;
                        *n
                    };
                    let kind = match answer(ttl, cycle) {
                        // the helper times out → `no-reply` → Sample::Lost (deviation 1)
                        Answer::NoReply => ResponseKind::NoReply,
                        Answer::Reply { addr, rtt_us, mpls } => ResponseKind::Probe {
                            result: if addr == e.target() {
                                ProbeResult::Reply
                            } else {
                                ProbeResult::TtlExpired
                            },
                            addr,
                            rtt_us,
                            mpls,
                        },
                    };
                    e.handle(Event::Probe { token, kind }, now);
                }
                Command::NextWake(t) => next = Some(t),
                Command::Finished => done = true,
                Command::Resolve(_) => {}
            }
        }
        if done {
            return (e, now);
        }
        now = next.expect("wake");
    }
}

pub struct Fixture {
    pub engine: Engine,
    pub names: NameCache,
    pub ui: UiState,
    pub palette: Palette,
    pub glyphs: &'static Glyphs,
    pub now: Instant,
    pub version: &'static str,
}

impl Fixture {
    pub fn view(&self) -> View<'_> {
        View {
            engine: &self.engine,
            names: &self.names,
            ui: &self.ui,
            glyphs: self.glyphs,
            palette: &self.palette,
            now: self.now,
            clock: "12:34:56",
            local_hostname: "testhost",
            target_name: "target.example",
            version: self.version,
        }
    }

    /// Defaults around an engine the test built itself (Task 11's ECMP cases).
    pub fn around(engine: Engine, now: Instant) -> Fixture {
        Fixture {
            engine,
            names: NameCache::default(),
            ui: UiState::new(),
            palette: Palette::new(Depth::Ansi16),
            glyphs: &UNICODE,
            now,
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

/// hop 1 = 10.0.0.1 @ 0.5 ms, hop 2 lost, hop 3 = target @ 2.5 ms; two cycles (like tests/emit.rs).
pub fn view_fixture() -> Fixture {
    let cfg = Config {
        max_ping: 2,
        force_max_ping: true,
        grace_time: 0.1,
        ..Config::default()
    };
    let (engine, end) = drive(cfg, |ttl, _| match ttl {
        1 => Answer::Reply {
            addr: ip("10.0.0.1"),
            rtt_us: 500,
            mpls: vec![],
        },
        2 => Answer::NoReply,
        _ => Answer::Reply {
            addr: ip("192.0.2.10"),
            rtt_us: 2500,
            mpls: vec![],
        },
    });
    let mut names = NameCache::default();
    names.insert_name(ip("10.0.0.1"), "gw.example");
    Fixture {
        engine,
        names,
        ui: UiState::new(),
        palette: Palette::new(Depth::Ansi16),
        glyphs: &UNICODE,
        now: end + Duration::from_secs(1),
        version: env!("CARGO_PKG_VERSION"),
    }
}

/// Task 13's snapshot scene: four cycles, hop 1 alternating 10.0.0.1 (0.5 ms) / 10.0.0.9 (0.9 ms,
/// one MPLS label), hop 2 lost, hop 3 = target with an ASN and an AS name; `-e` on.
pub fn snapshot_fixture(ascii: bool) -> Fixture {
    let cfg = Config {
        max_ping: 4,
        force_max_ping: true,
        grace_time: 0.1,
        mpls: true,
        ..Config::default()
    };
    let (engine, end) = drive(cfg, |ttl, cycle| match (ttl, cycle % 2) {
        (1, 0) => Answer::Reply {
            addr: ip("10.0.0.9"),
            rtt_us: 900,
            mpls: vec![MplsLabel {
                label: 100,
                tc: 0,
                bottom_of_stack: true,
                ttl: 1,
            }],
        },
        (1, _) => Answer::Reply {
            addr: ip("10.0.0.1"),
            rtt_us: 500,
            mpls: vec![],
        },
        (2, _) => Answer::NoReply,
        _ => Answer::Reply {
            addr: ip("192.0.2.10"),
            rtt_us: 2500,
            mpls: vec![],
        },
    });
    let mut names = NameCache::default();
    names.insert_name(ip("10.0.0.1"), "gw.example");
    let info = crate::asn::parse_txt("64500 | 192.0.2.0/24 | EX | ripe | 2020-01-01");
    // AsnInfo::name lands in Task 12; until then the AS name column falls back to field(0).
    names.insert_asn(ip("192.0.2.10"), info);
    Fixture {
        engine,
        names,
        ui: UiState::new(),
        palette: Palette::new(if ascii { Depth::Mono } else { Depth::Ansi16 }),
        glyphs: Glyphs::select(ascii),
        now: end + Duration::from_secs(2),
        // pinned so a version bump does not churn every snapshot
        version: "0.1.0",
    }
}

pub fn row_text(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width)
        .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
        .collect()
}
