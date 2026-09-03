// Golden tests for the ui/report.c layouts (mtr 0.96, commit 7b01773). GPL-2.0-only.

use std::net::IpAddr;
use std::time::Instant;

use mtr::asn::parse_txt;
use mtr::emit::{ReportContext, csv, json, report};
use mtr::names::NameCache;
use mtr_core::fields::active_fields;
use mtr_core::{Command, Config, Engine, Event};
use mtr_proto::{MplsLabel, ProbeResult, ResponseKind};

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

/// Same helper as crates/mtr-core/tests/scenarios.rs (integration tests cannot share code).
fn run_to_finish(e: &mut Engine, t0: Instant, mut answer: impl FnMut(u8) -> Option<(IpAddr, u32)>) {
    let mut now = t0;
    for _ in 0..100_000 {
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
                Command::Finished => return,
                Command::Resolve(_) => {}
            }
        }
        now = next.expect("a running engine always schedules a wake");
    }
    panic!("engine did not finish within 100000 ticks");
}

/// hop 1 = 10.0.0.1 @ 0.5 ms, hop 2 lost, hop 3 = target @ 2.5 ms; two cycles.
fn finished_engine(cfg: Config) -> Engine {
    let t0 = Instant::now();
    let mut e = Engine::new(cfg, ip("192.0.2.10"), Some(ip("192.0.2.1")), t0, 1);
    run_to_finish(&mut e, t0, |ttl| match ttl {
        1 => Some((ip("10.0.0.1"), 500)),
        2 => None,
        _ => Some((ip("192.0.2.10"), 2500)),
    });
    e
}

fn base_cfg() -> Config {
    Config {
        interactive: false,
        max_ping: 2,
        ..Config::default()
    }
}

fn ctx<'a>(e: &'a Engine, names: &'a NameCache, wide: bool) -> ReportContext<'a> {
    ReportContext {
        engine: e,
        names,
        local_hostname: "testhost",
        target_name: "target.example",
        wide,
        fields: active_fields(&e.config().fields),
    }
}

#[test]
fn report_matches_report_c_layout() {
    let e = finished_engine(base_cfg());
    let names = NameCache::default();
    let out = report::render(&ctx(&e, &names, false));
    let expected = [
        format!(
            "HOST: testhost{}   Loss%   Snt   Last   Avg  Best  Wrst StDev",
            " ".repeat(19)
        ),
        format!(
            "  1.|-- 10.0.0.1{}   0.00%     2    0.5   0.5   0.5   0.5   0.0",
            " ".repeat(17)
        ),
        format!(
            "  2.|-- ???{} 100.00%     2    0.0   0.0   0.0   0.0   0.0",
            " ".repeat(22)
        ),
        format!(
            "  3.|-- 192.0.2.10{}   0.00%     2    2.5   2.5   2.5   2.5   0.0",
            " ".repeat(15)
        ),
    ];
    assert_eq!(out, format!("{}\n", expected.join("\n")));
    // stats begin at column 33 on every line (report.c: stat_start = 33): the title "Loss%"
    // occupies 36..41 and every " %6.2f%%" value 33..41, so the '%' is always at column 40
    for line in out.lines() {
        assert_eq!(line.find('%'), Some(40), "{line:?}");
    }
}

#[test]
fn wide_report_appends_after_the_longest_name() {
    let e = finished_engine(base_cfg());
    let names = NameCache::default();
    let out = report::render(&ctx(&e, &names, true));
    let expected = [
        "HOST: testhost     Loss%   Snt   Last   Avg  Best  Wrst StDev",
        "  1.|-- 10.0.0.1     0.00%     2    0.5   0.5   0.5   0.5   0.0",
        "  2.|-- ???        100.00%     2    0.0   0.0   0.0   0.0   0.0",
        "  3.|-- 192.0.2.10   0.00%     2    2.5   2.5   2.5   2.5   0.0",
    ];
    assert_eq!(out, format!("{}\n", expected.join("\n")));
}

#[test]
fn report_with_names_show_ips_and_asn() {
    let mut cfg = base_cfg();
    cfg.show_ips = true;
    cfg.ipinfo_fields = vec![0];
    let e = finished_engine(cfg);
    let mut names = NameCache::default();
    names.insert_name(ip("10.0.0.1"), "gw.example");
    names.insert_asn(
        ip("10.0.0.1"),
        parse_txt("64500 | 10.0.0.0/8 | US | arin | 2000-01-01"),
    );
    let out = report::render(&ctx(&e, &names, false));
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("HOST: testhost"));
    assert_eq!(
        lines[0].find("Loss%"),
        Some(33 + 14 + 3),
        "stats start after the 14-wide AS column"
    );
    assert!(
        lines[1].starts_with("  1. AS64500       gw.example (10.0.0.1)"),
        "{:?}",
        lines[1]
    );
    assert!(
        lines[2].starts_with("  2. AS???         ???"),
        "{:?}",
        lines[2]
    );
    assert!(
        lines[3].starts_with("  3. AS???         192.0.2.10"),
        "{:?}",
        lines[3]
    );
    assert_eq!(lines[1].find("0.00%"), Some(33 + 14 + 3), "{:?}", lines[1]);
}

#[test]
fn report_lists_extra_ecmp_addresses_once() {
    let t0 = Instant::now();
    // one probe to ttl 1 per cycle: the first reply comes from 10.0.0.1, the second from 10.0.0.2
    let mut e = Engine::new(
        Config {
            interactive: false,
            max_ping: 2,
            max_ttl: 1,
            ..Config::default()
        },
        ip("192.0.2.10"),
        None,
        t0,
        1,
    );
    let mut n = 0;
    run_to_finish(&mut e, t0, |_| {
        n += 1;
        Some((
            if n == 1 {
                ip("10.0.0.1")
            } else {
                ip("10.0.0.2")
            },
            500,
        ))
    });
    let names = NameCache::default();
    let out = report::render(&ctx(&e, &names, false));
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines[1].starts_with("  1.|-- 10.0.0.2"),
        "latest responder is the primary: {:?}",
        lines[1]
    );
    assert_eq!(lines[2].trim_end(), "        10.0.0.1");
    assert_eq!(lines.len(), 3);
}

#[test]
fn json_matches_jansson_output() {
    let e = finished_engine(base_cfg());
    let names = NameCache::default();
    let out = json::render(&ctx(&e, &names, false));
    let expected = r#"{
    "report": {
        "mtr": {
            "src": "testhost",
            "dst": "target.example",
            "tos": 0,
            "tests": 2,
            "psize": "64",
            "bitpattern": "0x00"
        },
        "hubs": [
            {
                "count": 1,
                "host": "10.0.0.1",
                "Loss%": 0.0,
                "Snt": 2,
                "Last": 0.5,
                "Avg": 0.5,
                "Best": 0.5,
                "Wrst": 0.5,
                "StDev": 0.0
            },
            {
                "count": 2,
                "host": "???",
                "Loss%": 100.0,
                "Snt": 2,
                "Last": 0.0,
                "Avg": 0.0,
                "Best": 0.0,
                "Wrst": 0.0,
                "StDev": 0.0
            },
            {
                "count": 3,
                "host": "192.0.2.10",
                "Loss%": 0.0,
                "Snt": 2,
                "Last": 2.5,
                "Avg": 2.5,
                "Best": 2.5,
                "Wrst": 2.5,
                "StDev": 0.0
            }
        ]
    }
}
"#;
    assert_eq!(out, expected);
}

#[test]
fn json_reals_follow_precision_5_g_with_forced_decimal() {
    assert_eq!(json::json_real(0.0), "0.0");
    assert_eq!(json::json_real(0.5), "0.5");
    assert_eq!(json::json_real(100.0), "100.0");
    assert_eq!(json::json_real(12.345678), "12.346");
    assert_eq!(json::json_real(1234.5678), "1234.6");
    assert_eq!(json::json_real(123456.7), "1.2346e+05");
    assert_eq!(json::json_real(99999.6), "1e+05");
    assert_eq!(json::json_real(0.00001234), "1.234e-05");
    assert_eq!(
        json::json_string("a\"b\\c\n\u{1}"),
        "\"a\\\"b\\\\c\\n\\u0001\""
    );
}

#[test]
fn json_random_size_pattern_and_asn() {
    let mut cfg = base_cfg();
    cfg.packet_size = -100;
    cfg.bit_pattern = -1;
    cfg.ipinfo_fields = vec![0];
    cfg.fields = "L".to_string();
    let e = finished_engine(cfg);
    let mut names = NameCache::default();
    names.insert_asn(
        ip("10.0.0.1"),
        parse_txt("64500 | 10.0.0.0/8 | US | arin | 2000-01-01"),
    );
    let out = json::render(&ctx(&e, &names, false));
    assert!(out.contains("\"psize\": \"rand(28-100)\""), "{out}");
    assert!(out.contains("\"bitpattern\": \"rand(0x00-FF)\""), "{out}");
    assert!(out.contains("\"host\": \"10.0.0.1\",\n                \"ASN\": \"AS64500\",\n                \"Loss%\": 0.0\n"), "{out}");
    assert!(out.contains("\"ASN\": \"AS???\""), "{out}");
}

#[test]
fn csv_matches_csv_close() {
    let e = finished_engine(base_cfg());
    let names = NameCache::default();
    let out = csv::render(&ctx(&e, &names, false), 1_700_000_000);
    let v = env!("CARGO_PKG_VERSION");
    let expected = format!(
        "Mtr_Version,Start_Time,Status,Host,Hop,Ip,Loss%,Snt,,Last,Avg,Best,Wrst,StDev\n\
         MTR.{v},1700000000,OK,target.example,1,10.0.0.1,0.00,2,,0.50,0.50,0.50,0.50,0.00\n\
         MTR.{v},1700000000,OK,target.example,2,???,100.00,2,,0.00,0.00,0.00,0.00,0.00\n\
         MTR.{v},1700000000,OK,target.example,3,192.0.2.10,0.00,2,,2.50,2.50,2.50,2.50,0.00\n"
    );
    assert_eq!(out, expected);
}

#[test]
fn csv_adds_asn_column_and_wide_ecmp_rows() {
    let t0 = Instant::now();
    let mut cfg = Config {
        interactive: false,
        max_ping: 2,
        max_ttl: 1,
        ..Config::default()
    };
    cfg.ipinfo_fields = vec![0];
    cfg.fields = "LS".to_string();
    let mut e = Engine::new(cfg, ip("192.0.2.10"), None, t0, 1);
    let mut n = 0;
    run_to_finish(&mut e, t0, |_| {
        n += 1;
        Some((
            if n == 1 {
                ip("10.0.0.1")
            } else {
                ip("10.0.0.2")
            },
            500,
        ))
    });
    let mut names = NameCache::default();
    names.insert_asn(
        ip("10.0.0.1"),
        parse_txt("64500 | 10.0.0.0/8 | US | arin | 2000-01-01"),
    );
    let v = env!("CARGO_PKG_VERSION");
    let narrow = csv::render(&ctx(&e, &names, false), 7);
    assert_eq!(
        narrow,
        format!(
            "Mtr_Version,Start_Time,Status,Host,Hop,Ip,Asn,Loss%,Snt\nMTR.{v},7,OK,target.example,1,10.0.0.2,AS???,0.00,2\n"
        )
    );
    let wide = csv::render(&ctx(&e, &names, true), 7);
    assert_eq!(
        wide,
        format!(
            "Mtr_Version,Start_Time,Status,Host,Hop,Ip,Asn,Loss%,Snt\n\
             MTR.{v},7,OK,target.example,1,10.0.0.2,AS???,0.00,2\n\
             MTR.{v},7,OK,target.example,1,10.0.0.1,AS64500,0.00,2\n"
        )
    );
}

#[test]
fn csv_has_no_header_when_there_is_nothing_to_report() {
    let t0 = Instant::now();
    let e = Engine::new(base_cfg(), ip("192.0.2.10"), None, t0, 1);
    let names = NameCache::default();
    let out = csv::render(&ctx(&e, &names, false), 0);
    assert_eq!(out, "");
}

#[test]
fn mpls_labels_are_printed_after_the_hop_row() {
    let t0 = Instant::now();
    let mut e = Engine::new(
        Config {
            interactive: false,
            max_ping: 1,
            max_ttl: 1,
            mpls: true,
            ..Config::default()
        },
        ip("192.0.2.10"),
        None,
        t0,
        1,
    );
    let cmds = e.handle(Event::Tick, t0);
    let token = cmds
        .into_iter()
        .find_map(|c| match c {
            Command::SendProbe { token, .. } => Some(token),
            _ => None,
        })
        .expect("engine sends a probe on the first tick");
    e.handle(
        Event::Probe {
            token,
            kind: ResponseKind::Probe {
                result: ProbeResult::Reply,
                addr: e.target(),
                rtt_us: 500,
                mpls: vec![MplsLabel {
                    label: 16001,
                    tc: 0,
                    bottom_of_stack: true,
                    ttl: 1,
                }],
            },
        },
        t0,
    );
    e.end_transit();
    let names = NameCache::default();
    let out = report::render(&ctx(&e, &names, false));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[2], "       [MPLS: Lbl 16001 TC 0 S 1 TTL 1]", "{out}");
}

#[test]
fn start_line_uses_iso_time_with_numeric_offset() {
    let z: jiff::Zoned = "2026-09-02T12:41:07+02:00[+02:00]".parse().unwrap();
    assert_eq!(report::start_line(&z), "Start: 2026-09-02T12:41:07+0200");
}

#[test]
fn wide_report_pads_by_display_width_not_chars() {
    let e = finished_engine(base_cfg());
    let mut names = NameCache::default();
    names.insert_name(ip("10.0.0.1"), "日本.example");
    let out = report::render(&ctx(&e, &names, true));
    // every stats block starts at the same column: the Loss% '%' must line up on all rows
    let cols: Vec<usize> = out
        .lines()
        .skip(1)
        .map(|l| mtr::width::display_width(&l[..l.find('%').unwrap()]))
        .collect();
    assert!(cols.iter().all(|c| *c == cols[0]), "{out}");
}
