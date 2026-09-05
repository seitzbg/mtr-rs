//! Loopback and byte-level tests for the Unix backend (send_probe/receive round trips,
//! error paths, MPLS decode). Split out of mod.rs; ported from packet/probe_unix.c (mtr 0.96,
//! commit 7b01773). GPL-2.0-only.

use super::*;
use mtr_proto::{CProbeParams, ProbeResult, Protocol, ResponseKind};
use std::net::IpAddr;
use std::time::Duration;

/// Every backend in this process shares one `icmp_id` (our pid), and the kernel hands all
/// echo replies carrying an id to a single one of the ping sockets bound to it — so two
/// loopback tests running at once would steal each other's replies. That is a test-only
/// artifact (a real mtr-packet is one process with one backend), so the tests take turns.
static PROBE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A `UnixBackend` that holds the turn for as long as the test body does.
struct TestBackend {
    backend: UnixBackend,
    _turn: std::sync::MutexGuard<'static, ()>,
}

impl std::ops::Deref for TestBackend {
    type Target = UnixBackend;
    fn deref(&self) -> &UnixBackend {
        &self.backend
    }
}

impl std::ops::DerefMut for TestBackend {
    fn deref_mut(&mut self) -> &mut UnixBackend {
        &mut self.backend
    }
}

/// A real backend for the loopback tests, or `None` when this machine has neither
/// `cap_net_raw` nor open ping sockets for `version` (Linux), or is not root (FreeBSD) — then
/// the caller returns early instead of failing (Global Constraints). Every test below starts with
/// `let Some(mut b) = backend(4) else { return };`.
fn backend(version: u8) -> Option<TestBackend> {
    let turn = PROBE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !sockets::probe_sockets_available(version) {
        eprintln!(
            "skipping: no IPv{version} probe sockets (need cap_net_raw, ping sockets or root)"
        );
        return None;
    }
    let mut backend = UnixBackend::open_privileged().ok()?;
    backend.finish_init().unwrap();
    Some(TestBackend {
        backend,
        _turn: turn,
    })
}

/// Poll the backend until `pred` holds or `budget` elapses.
fn pump(
    b: &mut UnixBackend,
    t: &mut ProbeTable,
    budget: Duration,
    mut pred: impl FnMut(&[Response]) -> bool,
) -> Vec<Response> {
    let mut out = Vec::new();
    let start = Instant::now();
    while start.elapsed() < budget && !pred(&out) {
        std::thread::sleep(Duration::from_millis(10));
        let now = Instant::now();
        b.receive(t, now, &mut out);
        t.expire(now, &mut out);
    }
    out
}

#[test]
fn icmpv4_echo_to_loopback_gets_a_reply() {
    let Some(mut b) = backend(4) else { return };
    let mut t = ProbeTable::new();
    let p = CProbeParams {
        ip_version: 4,
        remote_address: Some("127.0.0.1".into()),
        ..Default::default()
    };
    let i = t.alloc(14, Instant::now(), 5).unwrap();
    b.send_probe(&mut t, i, &p).unwrap();
    assert_eq!(
        t.probes[i].remote.ip(),
        "127.0.0.1".parse::<IpAddr>().unwrap()
    );
    let out = pump(&mut b, &mut t, Duration::from_secs(3), |o| !o.is_empty());
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0].token, 14);
    match &out[0].kind {
        ResponseKind::Probe {
            result: ProbeResult::Reply,
            addr,
            rtt_us,
            mpls,
        } => {
            assert_eq!(*addr, "127.0.0.1".parse::<IpAddr>().unwrap());
            assert!(*rtt_us < 1_000_000);
            assert!(mpls.is_empty());
        }
        other => panic!("{other:?}"),
    }
    assert!(t.is_empty());
}

/// probe.h:40 + construct_unix.c:847-850: a `size` that does not fit C's 9000-byte packet
/// buffer is EINVAL — `invalid-argument` on the wire — and never allocates a buffer or
/// runs the checksum over one. `size 9000` is still accepted.
#[test]
fn sizes_beyond_the_packet_buffer_are_invalid_arguments() {
    let Some(mut b) = backend(4) else { return };
    let mut t = ProbeTable::new();
    let params = |size: i32, pattern: i32| CProbeParams {
        ip_version: 4,
        remote_address: Some("127.0.0.1".into()),
        packet_size: size,
        bit_pattern: pattern,
        ..Default::default()
    };
    for (size, pattern) in [(9001, 1), (200_000, 255), (i32::MAX, 1)] {
        let i = t.alloc(7, Instant::now(), 1).unwrap();
        let err = b
            .send_probe(&mut t, i, &params(size, pattern))
            .expect_err("size beyond PACKET_BUFFER_SIZE must fail");
        assert_eq!(err.raw_os_error(), Some(nix::libc::EINVAL), "size {size}");
        t.remove(i);
    }
    let i = t.alloc(8, Instant::now(), 5).unwrap();
    b.send_probe(&mut t, i, &params(9000, 255))
        .expect("9000 bytes is exactly the buffer size");
    let out = pump(&mut b, &mut t, Duration::from_secs(3), |o| !o.is_empty());
    assert!(!out.is_empty(), "a 9000-byte echo to loopback is answered");
}

#[test]
fn icmpv4_to_a_blackhole_times_out_with_no_reply() {
    let Some(mut b) = backend(4) else { return };
    let mut t = ProbeTable::new();
    // 192.0.2.0/24 (TEST-NET-1) is never routed back; a 1 s timeout ends it.
    let p = CProbeParams {
        ip_version: 4,
        remote_address: Some("192.0.2.1".into()),
        timeout: 1,
        ..Default::default()
    };
    let i = t.alloc(15, Instant::now(), 1).unwrap();
    match b.send_probe(&mut t, i, &p) {
        Ok(()) => {
            let out = pump(&mut b, &mut t, Duration::from_secs(3), |o| !o.is_empty());
            assert_eq!(
                out,
                vec![Response {
                    token: 15,
                    kind: ResponseKind::NoReply
                }]
            );
        }
        Err(e) => {
            // No default route in the sandbox. `sendto()` reports its own errno; a
            // `find_source_addr()` failure is flattened to EINVAL, as C reports a bare
            // `invalid-argument` there (probe.c:333-404, probe_unix.c:571-575).
            assert!(
                matches!(
                    e.raw_os_error(),
                    Some(nix::libc::ENETUNREACH | nix::libc::EHOSTUNREACH | nix::libc::EINVAL)
                ),
                "{e}"
            );
        }
    }
}

#[test]
fn ttl_one_to_loopback_is_still_a_reply_and_bad_ttl_is_invalid() {
    let Some(mut b) = backend(4) else { return };
    let mut t = ProbeTable::new();
    let p = CProbeParams {
        ip_version: 4,
        remote_address: Some("127.0.0.1".into()),
        ttl: 1,
        ..Default::default()
    };
    let i = t.alloc(16, Instant::now(), 5).unwrap();
    b.send_probe(&mut t, i, &p).unwrap();
    let out = pump(&mut b, &mut t, Duration::from_secs(3), |o| !o.is_empty());
    assert!(
        matches!(
            out[0].kind,
            ResponseKind::Probe {
                result: ProbeResult::Reply,
                ..
            }
        ),
        "{out:?}"
    );
    let bad = CProbeParams {
        ip_version: 4,
        remote_address: Some("127.0.0.1".into()),
        ttl: 300,
        ..Default::default()
    };
    let i = t.alloc(17, Instant::now(), 5).unwrap();
    let e = b.send_probe(&mut t, i, &bad).unwrap_err();
    assert_eq!(e.raw_os_error(), Some(nix::libc::EINVAL));
    t.remove(i);
}

#[test]
fn icmpv6_echo_to_loopback_gets_a_reply_with_the_full_address() {
    let Some(mut b) = backend(6) else { return };
    // Loopback IPv6 only — this box has no global unicast address (see Box facts).
    assert!(b.ip_version_supported(6));
    let mut t = ProbeTable::new();
    let p = CProbeParams {
        ip_version: 6,
        remote_address: Some("::1".into()),
        ..Default::default()
    };
    let i = t.alloc(52, Instant::now(), 5).unwrap();
    b.send_probe(&mut t, i, &p).unwrap();
    let out = pump(&mut b, &mut t, Duration::from_secs(3), |o| !o.is_empty());
    match &out[0].kind {
        ResponseKind::Probe {
            result: ProbeResult::Reply,
            addr,
            ..
        } => assert_eq!(addr.to_string(), "::1"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        out[0].encode(),
        format!(
            "52 reply ip-6 ::1 round-trip-time {}\n",
            match &out[0].kind {
                ResponseKind::Probe { rtt_us, .. } => rtt_us,
                _ => unreachable!(),
            }
        )
    );
}

#[test]
fn icmpv6_documentation_prefix_times_out_or_is_unroutable() {
    let Some(mut b) = backend(6) else { return };
    let mut t = ProbeTable::new();
    let p = CProbeParams {
        ip_version: 6,
        remote_address: Some("2001:db8::1".into()),
        timeout: 1,
        ..Default::default()
    };
    let i = t.alloc(53, Instant::now(), 1).unwrap();
    match b.send_probe(&mut t, i, &p) {
        Ok(()) => {
            let out = pump(&mut b, &mut t, Duration::from_secs(3), |o| !o.is_empty());
            assert!(
                matches!(
                    out[0].kind,
                    ResponseKind::NoReply
                        | ResponseKind::Probe {
                            result: ProbeResult::NoRouteHost,
                            ..
                        }
                ),
                "{out:?}"
            );
        }
        Err(e) => assert!(
            matches!(
                e.raw_os_error(),
                Some(nix::libc::ENETUNREACH | nix::libc::EHOSTUNREACH | nix::libc::EINVAL)
            ),
            "{e}"
        ),
    }
}

fn udp(remote: &str, version: u8, dest_port: i32, local_port: i32) -> CProbeParams {
    CProbeParams {
        ip_version: version,
        remote_address: Some(remote.into()),
        protocol: Protocol::Udp,
        dest_port,
        local_port,
        timeout: 3,
        ..Default::default()
    }
}

#[test]
fn udp_to_a_closed_loopback_port_is_a_reply_in_all_port_modes() {
    let Some(mut b) = backend(4) else { return };
    for (tok, params) in [
        (80, udp("127.0.0.1", 4, 0, 0)),
        (81, udp("127.0.0.1", 4, 990, 0)),
        (82, udp("127.0.0.1", 4, 0, 1991)),
        (83, udp("127.0.0.1", 4, 990, 1991)),
        (84, udp("::1", 6, 0, 0)),
    ] {
        if params.ip_version == 6 && !b.ip_version_supported(6) {
            continue; // no IPv6 probe sockets here
        }
        let mut t = ProbeTable::new();
        let i = t.alloc(tok, Instant::now(), 3).unwrap();
        b.send_probe(&mut t, i, &params).unwrap();
        let out = pump(&mut b, &mut t, Duration::from_secs(3), |o| !o.is_empty());
        assert_eq!(out.len(), 1, "token {tok}: {out:?}");
        assert!(
            matches!(
                out[0].kind,
                ResponseKind::Probe {
                    result: ProbeResult::Reply,
                    ..
                }
            ),
            "token {tok}: {out:?}"
        );
        assert!(t.is_empty());
    }
}

fn tcp(remote: &str, version: u8, dest_port: i32) -> CProbeParams {
    CProbeParams {
        ip_version: version,
        remote_address: Some(remote.into()),
        protocol: Protocol::Tcp,
        dest_port,
        timeout: 3,
        ..Default::default()
    }
}

/// Both halves of the local case: a listening port answers with SYN/ACK and a closed one
/// with RST, and C calls each of them `reply` (probe_unix.c:896-903). The third case, a
/// ttl-1 probe to a routed address answered by an ICMP time-exceeded, needs the raw receive
/// socket to see an ICMP quoting *our* SYN, which an unprivileged process never gets — so
/// it is not testable here and is left to `probe.py`'s `TestProbeTCP` under `cap_net_raw`.
#[test]
fn tcp_to_an_open_and_a_closed_loopback_port_both_reply() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let open_port = i32::from(listener.local_addr().unwrap().port());
    let Some(mut b) = backend(4) else { return };
    for (tok, port) in [(80, open_port), (81, 1)] {
        let mut t = ProbeTable::new();
        let i = t.alloc(tok, Instant::now(), 3).unwrap();
        match b.send_probe(&mut t, i, &tcp("127.0.0.1", 4, port)) {
            Ok(()) => {
                assert!(t.probes[i].stream.is_some(), "the probe owns its socket");
                assert_eq!(t.stream_fds().len(), 1, "and it is polled for POLLOUT");
                let out = pump(&mut b, &mut t, Duration::from_secs(3), |o| !o.is_empty());
                assert!(
                    matches!(
                        out[0].kind,
                        ResponseKind::Probe {
                            result: ProbeResult::Reply,
                            ..
                        }
                    ),
                    "token {tok}: {out:?}"
                );
                assert_eq!(out.len(), 1, "token {tok}: {out:?}");
                assert!(t.is_empty(), "the socket is closed with the probe");
                assert!(t.stream_fds().is_empty());
            }
            Err(e) => {
                // Loopback may refuse synchronously; the dispatcher turns this into a reply
                // (probe_unix.c:610-620, command.rs).
                assert_eq!(
                    e.raw_os_error(),
                    Some(nix::libc::ECONNREFUSED),
                    "token {tok}"
                );
                assert_eq!(
                    t.probes[i].remote.ip(),
                    "127.0.0.1".parse::<IpAddr>().unwrap(),
                    "remote must be set before the error"
                );
            }
        }
    }
    drop(listener);
}

/// probe_unix.c:588-608: a source port that cannot be bound is not fatal — the probe takes
/// the next sequence number and tries again. The blocker is a plain `TcpListener`, which
/// sets `SO_REUSEADDR` but not `SO_REUSEPORT`, so our probe socket cannot share its port.
#[test]
fn tcp_retries_the_next_sequence_when_the_source_port_is_taken() {
    let Some(mut b) = backend(4) else { return };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut t = ProbeTable::new();
    let i = t.alloc(90, Instant::now(), 3).unwrap();
    let seq = t.probes[i].sequence;
    // Occupy local:seq so the first attempt's bind() fails with EADDRINUSE. If the bind
    // loses to a sibling socket that set SO_REUSEPORT, the port is not reliably blocked and
    // the retry under test would not be triggered, so give up rather than fail spuriously.
    let Ok(blocker) = std::net::TcpListener::bind(("127.0.0.1", seq)) else {
        return;
    };
    let dest = i32::from(listener.local_addr().unwrap().port());
    b.send_probe(&mut t, i, &tcp("127.0.0.1", 4, dest)).unwrap();
    assert_ne!(
        t.probes[i].sequence, seq,
        "the sequence advanced past the busy port"
    );
    let bound = t.probes[i]
        .stream
        .as_ref()
        .unwrap()
        .local_addr()
        .unwrap()
        .as_socket()
        .unwrap()
        .port();
    assert_eq!(bound, t.probes[i].sequence, "bound to the new sequence");
    assert_eq!(t.probes[i].local.port(), t.probes[i].sequence);
    assert_eq!(t.probes[i].remote.port() as i32, dest);
    let out = pump(&mut b, &mut t, Duration::from_secs(3), |o| !o.is_empty());
    assert!(
        matches!(
            out[0].kind,
            ResponseKind::Probe {
                result: ProbeResult::Reply,
                ..
            }
        ),
        "{out:?}"
    );
    drop(blocker);
}

#[test]
fn udp_probe_records_the_ports_it_used() {
    let Some(mut b) = backend(4) else { return };
    let mut t = ProbeTable::new();
    let i = t.alloc(85, Instant::now(), 3).unwrap();
    b.send_probe(&mut t, i, &udp("127.0.0.1", 4, 990, 1991))
        .unwrap();
    assert_eq!(
        (t.probes[i].local.port(), t.probes[i].remote.port()),
        (1991, 990)
    );
    let i2 = t.alloc(86, Instant::now(), 3).unwrap();
    b.send_probe(&mut t, i2, &udp("127.0.0.1", 4, 0, 0))
        .unwrap();
    assert_eq!(t.probes[i2].remote.port(), t.probes[i2].sequence);
}

fn sctp(remote: &str, version: u8, dest_port: i32) -> CProbeParams {
    CProbeParams {
        ip_version: version,
        remote_address: Some(remote.into()),
        protocol: Protocol::Sctp,
        dest_port,
        timeout: 3,
        ..Default::default()
    }
}

#[test]
fn an_sctp_probe_without_kernel_sctp_is_an_invalid_argument_not_an_unexpected_error() {
    let Some(mut b) = backend(4) else { return };
    b.sctp = false; // pretend the module is absent, as on a stripped kernel
    let mut t = ProbeTable::new();
    let i = t.alloc(94, Instant::now(), 3).unwrap();
    let p = CProbeParams {
        ip_version: 4,
        remote_address: Some("127.0.0.1".into()),
        protocol: Protocol::Sctp,
        dest_port: 164,
        timeout: 3,
        ..Default::default()
    };
    let e = b.send_probe(&mut t, i, &p).unwrap_err();
    // `is_protocol_supported()` (probe_unix.c:506-528) rejects it; EINVAL is what
    // `error_response` turns into `invalid-argument`, never `unexpected-error errno 93`.
    assert_eq!(e.raw_os_error(), Some(nix::libc::EINVAL));
    t.remove(i);
}

#[test]
fn sctp_to_a_closed_loopback_port_replies_when_supported() {
    let Some(mut b) = backend(4) else { return };
    if !b.protocol_supported(Protocol::Sctp) {
        eprintln!("skipping: no SCTP support");
        return;
    }
    let mut t = ProbeTable::new();
    let i = t.alloc(95, Instant::now(), 3).unwrap();
    let p = CProbeParams {
        ip_version: 4,
        remote_address: Some("127.0.0.1".into()),
        protocol: Protocol::Sctp,
        dest_port: 164,
        timeout: 3,
        ..Default::default()
    };
    match b.send_probe(&mut t, i, &p) {
        Ok(()) => {
            let out = pump(&mut b, &mut t, Duration::from_secs(3), |o| !o.is_empty());
            assert!(
                matches!(
                    out[0].kind,
                    ResponseKind::Probe {
                        result: ProbeResult::Reply,
                        ..
                    }
                ),
                "{out:?}"
            );
        }
        Err(e) => assert_eq!(e.raw_os_error(), Some(nix::libc::ECONNREFUSED)),
    }
}

#[test]
fn sctp_to_a_closed_loopback_v6_port_replies_when_supported() {
    let Some(mut b) = backend(6) else { return };
    if !b.protocol_supported(Protocol::Sctp) {
        eprintln!("skipping: no SCTP support");
        return;
    }
    let mut t = ProbeTable::new();
    let i = t.alloc(96, Instant::now(), 3).unwrap();
    let p = sctp("::1", 6, 164);
    match b.send_probe(&mut t, i, &p) {
        Ok(()) => {
            let out = pump(&mut b, &mut t, Duration::from_secs(3), |o| !o.is_empty());
            assert!(
                matches!(
                    out[0].kind,
                    ResponseKind::Probe {
                        result: ProbeResult::Reply,
                        ..
                    }
                ),
                "{out:?}"
            );
        }
        Err(e) => assert_eq!(e.raw_os_error(), Some(nix::libc::ECONNREFUSED)),
    }
}

/// MILESTONE 3 (Task 15): the labels `decode_mpls()` lifted out of the RFC 4950 extension
/// have to survive `deliver()` -> `respond()` -> `Response::encode()` and appear as the
/// `mpls` argument of the `ttl-expired` line (probe.c:250-320 formats the reply, the
/// `mpls` key coming from `format_mpls_string()` at probe.c:212-244).
#[test]
fn mpls_labels_reach_the_reply_line() {
    let Some(b) = backend(4) else { return };
    let mut t = ProbeTable::new();
    let i = t.alloc(7, Instant::now(), 10).unwrap();
    t.probes[i].sequence = 33435;
    t.probes[i].remote = "8.8.8.8:0".parse().unwrap();
    t.probes[i].local = "10.0.0.2:0".parse().unwrap();
    let parsed = deconstruct::Parsed {
        kind: deconstruct::IcmpKind::TimeExceeded,
        echo: None,
        inner: Some(deconstruct::Inner::Icmp {
            id: b.icmp_id,
            sequence: 33435,
        }),
        mpls: vec![mtr_proto::MplsLabel {
            label: 16,
            tc: 0,
            bottom_of_stack: true,
            ttl: 255,
        }],
    };
    let mut out = Vec::new();
    b.deliver(
        &mut t,
        &parsed,
        4,
        "10.0.0.1".parse().unwrap(),
        Instant::now(),
        &mut out,
    );
    assert_eq!(out.len(), 1);
    let line = out[0].encode();
    assert!(
        line.starts_with("7 ttl-expired ip-4 10.0.0.1 round-trip-time "),
        "{line}"
    );
    assert!(line.trim_end().ends_with(" mpls 16,0,1,255"), "{line}");
    assert!(t.is_empty());
}

/// The same path starting from bytes: a synthetic ICMPv4 time-exceeded quoting our own echo
/// and carrying an RFC 4884/4950 extension (the Task 9 fixture shape) goes through
/// `parse_icmp4()` and comes out as one wire line. Also pins `MAX_MPLS_LABELS`
/// (deconstruct_unix.c:28) on the wire: a 10-label object yields 8 groups, and a reply with
/// no extension has no `mpls` key at all.
#[test]
fn synthetic_time_exceeded_bytes_produce_an_mpls_reply_line() {
    let Some(b) = backend(4) else { return };
    // ICMP time-exceeded header + the 128-byte quoted original datagram.
    let quoted = |id: u16| {
        let mut icmp = vec![11u8, 0, 0, 0, 0, 0, 0, 0];
        let mut inner = vec![
            0x45u8, 0, 0, 28, 0, 0, 0, 0, 64, 1, /* IPPROTO_ICMP */
            0, 0,
        ];
        inner.extend_from_slice(&[10, 0, 0, 2]);
        inner.extend_from_slice(&[8, 8, 8, 8]);
        inner.extend_from_slice(&[8, 0, 0, 0]);
        inner.extend_from_slice(&id.to_be_bytes());
        inner.extend_from_slice(&33435u16.to_be_bytes());
        inner.resize(128, 0);
        icmp.extend(inner);
        icmp
    };
    let with_labels = |n: u8| {
        let mut icmp = quoted(b.icmp_id);
        icmp.extend_from_slice(&[0x20, 0, 0, 0]); // extension header, version 2
        icmp.extend_from_slice(&[0, 4 + 4 * n, 1, 1]); // length, class 1 (MPLS), ctype 1
        for i in 0..n {
            // label 16 + i, tc 0, bottom-of-stack, ttl 255
            icmp.extend_from_slice(&[0x00, 0x01, 0x01 | (i << 4), 0xff]);
        }
        icmp
    };
    let encode = |icmp: &[u8]| {
        let mut t = ProbeTable::new();
        let i = t.alloc(7, Instant::now(), 10).unwrap();
        t.probes[i].sequence = 33435;
        t.probes[i].remote = "8.8.8.8:0".parse().unwrap();
        t.probes[i].local = "10.0.0.2:0".parse().unwrap();
        let parsed = parse_icmp4(icmp, false).unwrap();
        let mut out = Vec::new();
        b.deliver(
            &mut t,
            &parsed,
            4,
            "10.0.0.1".parse().unwrap(),
            Instant::now(),
            &mut out,
        );
        assert_eq!(out.len(), 1, "{out:?}");
        out[0].encode()
    };

    let line = encode(&with_labels(2));
    assert!(
        line.starts_with("7 ttl-expired ip-4 10.0.0.1 round-trip-time "),
        "{line}"
    );
    assert!(
        line.trim_end().ends_with(" mpls 16,0,1,255,17,0,1,255"),
        "{line}"
    );

    // MAX_MPLS_LABELS is 8, so a 10-label stack still puts exactly 8 groups on the wire.
    let capped = encode(&with_labels(10));
    let list = capped.split(" mpls ").nth(1).unwrap().trim_end();
    assert_eq!(
        list.split(',').count(),
        4 * deconstruct::MAX_MPLS_LABELS,
        "{capped}"
    );

    // No extension: no `mpls` key (Response::encode only appends it when labels exist,
    // as probe.c:299-303 does).
    let bare = encode(&quoted(b.icmp_id));
    assert!(
        bare.starts_with("7 ttl-expired ip-4 10.0.0.1 round-trip-time "),
        "{bare}"
    );
    assert!(!bare.contains(" mpls "), "{bare}");
}
