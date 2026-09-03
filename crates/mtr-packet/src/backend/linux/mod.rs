//! Linux backend: raw sockets with the unprivileged DGRAM fallback. Ported from
//! packet/probe_unix.c (mtr 0.96, commit 7b01773). GPL-2.0-only.

pub mod construct;
pub mod deconstruct;
pub mod errqueue;
pub mod sockets;

use std::net::{IpAddr, SocketAddr};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::time::Instant;

use mtr_proto::{CProbeParams, MplsLabel, ProbeResult, Protocol, Response, ResponseKind};
use socket2::{SockAddr, Socket};

use super::ProbeBackend;
use crate::Fatal;
use crate::probe_table::{Probe, ProbeTable, addr, rtt_us};
use construct::{icmp_echo, packet_size, udp_datagram, udp_ports, udp_source_port_from_pid};
use deconstruct::{Inner, Parsed, match_reply, parse_icmp4, parse_icmp6};
use errqueue::QueuedError;
use sockets::{Family, apply_probe_options};

/// `PACKET_BUFFER_SIZE` (probe.h:40).
pub const PACKET_BUFFER_SIZE: usize = 9000;

pub struct LinuxBackend {
    pub v4: Option<Family>,
    pub v6: Option<Family>,
    pub sctp: bool,
    /// The ICMP id: the low 16 bits of our pid, kept in host order and byte-swapped when the
    /// echo header is serialized, so the value on the wire is the one C writes with
    /// `htons(getpid())` (construct_unix.c:118, probe.c:193).
    pub icmp_id: u16,
}

impl LinuxBackend {
    /// `init_net_state_privileged()` (probe_unix.c:422-459).
    pub fn open_privileged() -> Result<Self, Fatal> {
        let v4 = Family::open(4);
        let v6 = Family::open(6);
        if v4.is_err() && v6.is_err() {
            let e4 = v4.err().map(|e| e.to_string()).unwrap_or_default();
            let e6 = v6.err().map(|e| e.to_string()).unwrap_or_default();
            return Err(Fatal::Message(format!(
                "Failure to open IPv4 sockets: {e4}\nmtr-packet: Failure to open IPv6 sockets: {e6}"
            )));
        }
        Ok(LinuxBackend {
            v4: v4.ok(),
            v6: v6.ok(),
            sctp: false,
            icmp_id: (nix::unistd::getpid().as_raw() as u32 & 0xffff) as u16,
        })
    }

    /// `init_net_state()` (probe_unix.c:465-486), run after the privilege drop.
    pub fn finish_init(&mut self) -> std::io::Result<()> {
        for f in [&self.v4, &self.v6].into_iter().flatten() {
            f.set_nonblocking()?;
        }
        self.sctp = sockets::check_sctp_support();
        Ok(())
    }

    pub fn family(&self, version: u8) -> Option<&Family> {
        match version {
            4 => self.v4.as_ref(),
            6 => self.v6.as_ref(),
            _ => None,
        }
    }

    /// `resolve_probe_addresses()` (probe.c:95-129): the remote address from `ip_version` plus
    /// the request's text, the local one from `local-ip` or `find_source_addr()`. ICMP on a
    /// DGRAM socket gets local port = `icmp_id`, because the kernel puts the bound port in the
    /// echo id field for us (probe.c:119-126).
    fn resolve(&self, params: &CProbeParams) -> std::io::Result<(SocketAddr, SocketAddr)> {
        let einval = || std::io::Error::from_raw_os_error(nix::libc::EINVAL);
        let remote_ip = addr::decode(
            params.ip_version,
            params.remote_address.as_deref().ok_or_else(einval)?,
        )
        .ok_or_else(einval)?;
        let local_ip = match &params.local_address {
            Some(s) => addr::decode(params.ip_version, s).ok_or_else(einval)?,
            None => addr::find_source_addr(remote_ip)?,
        };
        let family = self.family(params.ip_version).ok_or_else(einval)?;
        let local_port = if params.protocol == Protocol::Icmp && !family.is_raw() {
            self.icmp_id
        } else {
            0
        };
        Ok((
            SocketAddr::new(remote_ip, 0),
            SocketAddr::new(local_ip, local_port),
        ))
    }

    /// `construct_packet()` + `send_packet()` for ICMP and UDP (construct_unix.c:832-872,
    /// probe_unix.c:47-123). The buffer starts out filled with the bit pattern
    /// (construct_unix.c:855) and each builder overwrites only its own header. Stream
    /// protocols never reach here; they go to `stream.rs` (Task 13).
    fn send_datagram(
        &self,
        family: &Family,
        probe: &mut Probe,
        params: &CProbeParams,
    ) -> std::io::Result<()> {
        let einval = || std::io::Error::from_raw_os_error(nix::libc::EINVAL);
        let size = packet_size(params, family.is_raw()).ok_or_else(einval)?;
        let mut buf = vec![params.bit_pattern as u8; size];
        let sock: &Socket;
        let mut dst = probe.remote;
        match params.protocol {
            Protocol::Icmp => {
                sock = family.icmp_send();
                icmp_echo(&mut buf, params.ip_version, self.icmp_id, probe.sequence);
            }
            Protocol::Udp => {
                sock = family.udp_send();
                let pid_port = udp_source_port_from_pid(nix::unistd::getpid().as_raw() as u32);
                let ports = udp_ports(params, probe.sequence, pid_port);
                // construct_unix.c:181-183: the chosen ports become the probe's own, which is
                // what `match_udp()` compares an inner UDP header against.
                probe.local.set_port(ports.src);
                probe.remote.set_port(ports.dst);
                udp_datagram(
                    &mut buf,
                    ports,
                    probe.sequence,
                    probe.local.ip(),
                    probe.remote.ip(),
                );
                // Raw: the port is in the payload we built. DGRAM: the kernel builds the UDP
                // header, so the destination port must be on the sockaddr instead
                // (probe_unix.c:70-84, 96-111).
                dst.set_port(if family.is_raw() { 0 } else { ports.dst });
            }
            Protocol::Tcp | Protocol::Sctp => return Err(einval()),
        }
        apply_probe_options(
            sock,
            params.ip_version,
            params,
            Some(probe.local),
            family.is_raw(),
        )?;
        sock.send_to(&buf, &SockAddr::from(dst))?;
        Ok(())
    }

    /// `respond_to_probe()` (probe.c:250-320): the probe leaves the table exactly once and its
    /// token gets exactly one reply.
    #[allow(clippy::too_many_arguments)]
    fn respond(
        &self,
        table: &mut ProbeTable,
        idx: usize,
        result: ProbeResult,
        from: IpAddr,
        now: Instant,
        mpls: Vec<MplsLabel>,
        out: &mut Vec<Response>,
    ) {
        let p = table.remove(idx);
        out.push(Response {
            token: p.token,
            kind: ResponseKind::Probe {
                result,
                addr: from,
                rtt_us: rtt_us(p.departure, now),
                mpls,
            },
        });
    }

    /// `find_and_receive_probe()` (deconstruct_unix.c:34-54): a parsed message that matches no
    /// outstanding probe is dropped silently, as in C.
    fn deliver(
        &self,
        table: &mut ProbeTable,
        parsed: &Parsed,
        version: u8,
        from: IpAddr,
        now: Instant,
        out: &mut Vec<Response>,
    ) {
        if let Some((idx, result)) = match_reply(table, parsed, version, self.icmp_id) {
            self.respond(table, idx, result, from, now, parsed.mpls.clone(), out);
        }
    }

    /// `receive_replies_from_recv_socket()` for one socket (probe_unix.c:704-844). `recvfrom`
    /// fills a plain `&mut [u8]`, so no `MaybeUninit`/`unsafe` is needed. Every errno other
    /// than `EAGAIN`, `EINTR`, `EHOSTUNREACH` and `ECONNREFUSED` is returned to the caller: C
    /// calls `error(EXIT_FAILURE, errno, "Failure receiving replies")` there
    /// (probe_unix.c:790), and so must we — a silently swallowed receive error would look like
    /// a network of timeouts.
    fn drain_socket(
        &self,
        sock: &Socket,
        family: &Family,
        table: &mut ProbeTable,
        now: Instant,
        out: &mut Vec<Response>,
    ) -> std::io::Result<()> {
        use nix::sys::socket::{SockaddrStorage, recvfrom};
        let version = family.version;
        let mut buf = vec![0u8; PACKET_BUFFER_SIZE];
        let mut err_buf = vec![0u8; PACKET_BUFFER_SIZE];
        loop {
            match recvfrom::<SockaddrStorage>(sock.as_raw_fd(), &mut buf) {
                Ok((n, from)) => {
                    let from_ip = from.and_then(|s| {
                        s.as_sockaddr_in()
                            .map(|a| IpAddr::V4(a.ip()))
                            .or_else(|| s.as_sockaddr_in6().map(|a| IpAddr::V6(a.ip())))
                    });
                    let Some(from_ip) = from_ip else { continue };
                    let parsed = if version == 6 {
                        parse_icmp6(&buf[..n])
                    } else {
                        parse_icmp4(&buf[..n], family.is_raw())
                    };
                    if let Some(p) = parsed {
                        self.deliver(table, &p, version, from_ip, now, out);
                    }
                }
                Err(nix::errno::Errno::EAGAIN) => return Ok(()),
                Err(nix::errno::Errno::EINTR) => continue,
                Err(e @ (nix::errno::Errno::EHOSTUNREACH | nix::errno::Errno::ECONNREFUSED)) => {
                    // The errno is only a first guess; Task 11 refines it from the cmsg.
                    let fallback = if e == nix::errno::Errno::EHOSTUNREACH {
                        QueuedError::TimeExceeded
                    } else {
                        QueuedError::Refused
                    };
                    // A failed error-queue read is fatal for the same reason; an empty queue
                    // (`Ok(None)`, i.e. EAGAIN on MSG_ERRQUEUE) just ends this socket's turn.
                    match errqueue::read_error(sock, &mut err_buf, fallback)? {
                        Some((offender, n, kind)) => self.deliver_queued(
                            sock,
                            table,
                            version,
                            offender,
                            &err_buf[..n],
                            kind,
                            now,
                            out,
                        ),
                        None => return Ok(()),
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// The result word an error-queue entry stands for, expressed as the `IcmpKind` the normal
    /// matcher already understands: `Refused` is a port-unreachable (code 3 on v4, 4 on v6) and
    /// therefore `reply` (pre-flight ruling 3), and anything else unreachable is
    /// `no-route-host`.
    fn queued_icmp_kind(kind: QueuedError, version: u8) -> deconstruct::IcmpKind {
        match kind {
            QueuedError::TimeExceeded => deconstruct::IcmpKind::TimeExceeded,
            QueuedError::Refused => deconstruct::IcmpKind::DestUnreach {
                code: deconstruct::port_unreach_code(version),
            },
            QueuedError::Unreachable => deconstruct::IcmpKind::DestUnreach { code: 0 },
        }
    }

    /// `handle_error_queue_packet()` (deconstruct_unix.c:127-146) — the ICMP half now, the UDP
    /// half in Task 12. C picks the branch from the send socket's `SO_PROTOCOL`
    /// (probe_unix.c:824-828); `Socket::protocol()` is the same `getsockopt`.
    #[allow(clippy::too_many_arguments)]
    fn deliver_queued(
        &self,
        sock: &Socket,
        table: &mut ProbeTable,
        version: u8,
        offender: IpAddr,
        payload: &[u8],
        kind: QueuedError,
        now: Instant,
        out: &mut Vec<Response>,
    ) {
        if matches!(sock.protocol(), Ok(Some(socket2::Protocol::UDP))) {
            return; // Task 12
        }
        if payload.len() < construct::ICMP_HEADER {
            return;
        }
        let inner = Inner::Icmp {
            id: u16::from_be_bytes([payload[4], payload[5]]),
            sequence: u16::from_be_bytes([payload[6], payload[7]]),
        };
        let parsed = Parsed {
            kind: Self::queued_icmp_kind(kind, version),
            echo: None,
            inner: Some(inner),
            mpls: Vec::new(),
        };
        // On the error queue the quoted ICMP id is whatever the kernel assigned, i.e. our bound
        // port, which is `icmp_id` — so `match_reply()`'s id check still holds. Answering at
        // all here is deviation 26(b): C decodes nothing else from the error queue.
        self.deliver(table, &parsed, version, offender, now, out);
    }
}

impl ProbeBackend for LinuxBackend {
    fn ip_version_supported(&self, version: u8) -> bool {
        self.family(version).is_some()
    }
    fn protocol_supported(&self, protocol: Protocol) -> bool {
        protocol != Protocol::Sctp || self.sctp
    }
    fn mark_supported(&self) -> bool {
        true
    }
    /// `send_probe()` (probe_unix.c:559-640). `timeout_at` stays as `alloc()` stamped it; C
    /// re-stamps departure just before constructing the packet (probe_unix.c:581) and we do the
    /// same, so the two differ only by the microseconds spent decoding the command.
    fn send_probe(
        &mut self,
        table: &mut ProbeTable,
        idx: usize,
        params: &CProbeParams,
    ) -> std::io::Result<()> {
        let (remote, local) = self.resolve(params)?;
        let family = self
            .family(params.ip_version)
            .ok_or_else(|| std::io::Error::from_raw_os_error(nix::libc::EINVAL))?;
        let probe = &mut table.probes[idx];
        probe.remote = remote;
        probe.local = local;
        probe.departure = Instant::now();
        match params.protocol {
            Protocol::Icmp | Protocol::Udp => self.send_datagram(family, probe, params),
            // Task 13.
            Protocol::Tcp | Protocol::Sctp => {
                Err(std::io::Error::from_raw_os_error(nix::libc::EINVAL))
            }
        }
    }
    fn recv_fds(&self) -> Vec<BorrowedFd<'_>> {
        [&self.v4, &self.v6]
            .into_iter()
            .flatten()
            .flat_map(Family::recv_fds)
            .collect()
    }
    fn receive(&mut self, table: &mut ProbeTable, now: Instant, out: &mut Vec<Response>) {
        // `ProbeBackend::receive` cannot report failure, and neither can C: an unexpected
        // receive errno is `error(EXIT_FAILURE, …, "Failure receiving replies")`
        // (probe_unix.c:790), so we print the same message and exit 1.
        for family in [&self.v4, &self.v6].into_iter().flatten() {
            let r = match &family.sockets {
                sockets::Sockets::Raw { recv, .. } => {
                    self.drain_socket(recv, family, table, now, out)
                }
                sockets::Sockets::Dgram { icmp, udp } => self
                    .drain_socket(icmp, family, table, now, out)
                    .and_then(|()| self.drain_socket(udp, family, table, now, out)),
            };
            if let Err(e) = r {
                eprintln!("mtr-packet: Failure receiving replies: {e}");
                std::process::exit(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtr_proto::{CProbeParams, ProbeResult, ResponseKind};
    use std::net::IpAddr;
    use std::time::Duration;

    /// Every backend in this process shares one `icmp_id` (our pid), and the kernel hands all
    /// echo replies carrying an id to a single one of the ping sockets bound to it — so two
    /// loopback tests running at once would steal each other's replies. That is a test-only
    /// artifact (a real mtr-packet is one process with one backend), so the tests take turns.
    static PROBE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A `LinuxBackend` that holds the turn for as long as the test body does.
    struct TestBackend {
        backend: LinuxBackend,
        _turn: std::sync::MutexGuard<'static, ()>,
    }

    impl std::ops::Deref for TestBackend {
        type Target = LinuxBackend;
        fn deref(&self) -> &LinuxBackend {
            &self.backend
        }
    }

    impl std::ops::DerefMut for TestBackend {
        fn deref_mut(&mut self) -> &mut LinuxBackend {
            &mut self.backend
        }
    }

    /// A real backend for the loopback tests, or `None` when this machine has neither
    /// `cap_net_raw` nor open ping sockets for `version` — then the caller returns early
    /// instead of failing (Global Constraints). Every test below starts with
    /// `let Some(mut b) = backend(4) else { return };`.
    fn backend(version: u8) -> Option<TestBackend> {
        let turn = PROBE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if !sockets::dgram_available(version) {
            eprintln!("skipping: no IPv{version} probe sockets (need cap_net_raw or ping sockets)");
            return None;
        }
        let mut backend = LinuxBackend::open_privileged().ok()?;
        backend.finish_init().unwrap();
        Some(TestBackend {
            backend,
            _turn: turn,
        })
    }

    /// Poll the backend until `pred` holds or `budget` elapses.
    fn pump(
        b: &mut LinuxBackend,
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
                // No default route in the sandbox: C reports the errno name; that is also fine.
                assert!(
                    matches!(
                        e.raw_os_error(),
                        Some(nix::libc::ENETUNREACH | nix::libc::EHOSTUNREACH)
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
                    Some(nix::libc::ENETUNREACH | nix::libc::EHOSTUNREACH)
                ),
                "{e}"
            ),
        }
    }
}
