//! Linux backend: raw sockets with the unprivileged DGRAM fallback. Ported from
//! packet/probe_unix.c (mtr 0.96, commit 7b01773). GPL-2.0-only.
//!
//! UDP destination ports (probe_unix.c:70-84, 96-111): on a raw socket the port lives in the
//! UDP header we built, so the sockaddr port is 0. On a DGRAM socket the kernel writes the
//! header, so the port has to be on the sockaddr — `dest_port` when the request gave one, else
//! the sequence. C stores the sequence there as a host-order `int` into a network-order field
//! (`*sockaddr_port_offset(&dst) = sequence`, probe_unix.c:83/109), so C's datagram goes to the
//! byte-swapped port; that is harmless for C because its reply matching only ever looks at the
//! fake UDP header travelling in the payload. We send to the sequence proper, so the target's
//! port-unreachable is about the port we meant; the fake header's destination port is the
//! sequence either way, so matching is unaffected.

pub mod construct;
pub mod deconstruct;
pub mod errqueue;
pub mod sockets;
pub mod stream;

use std::net::{IpAddr, SocketAddr};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::time::Instant;

use mtr_proto::{CProbeParams, MplsLabel, ProbeResult, Protocol, Response, ResponseKind};
use socket2::{SockAddr, Socket};

use super::ProbeBackend;
use crate::Fatal;
use crate::probe_table::{MAX_PORT, MIN_PORT, Probe, ProbeTable, addr, rtt_us};
use construct::{
    UDP_HEADER, icmp_echo, packet_size, udp_datagram, udp_ports, udp_source_port_from_pid,
};
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
    /// Set by `receive()` instead of exiting on the spot; see `ProbeBackend::take_fatal`.
    fatal: Option<Fatal>,
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
            fatal: None,
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
        let dst = SockAddr::from(dst);
        match sock.send_to(&buf, &dst) {
            Ok(_) => Ok(()),
            // On a DGRAM socket a queued ICMP error also sets `sk_err`, and the next syscall on
            // the socket — here the *next* probe's `sendto` — reports and clears it, dropping
            // the datagram. That errno belongs to an earlier probe, not this one, so retry
            // once; a genuinely unroutable destination fails again and is returned. C hits the
            // same wart and mis-reports the new probe (probe_unix.c:629-633).
            Err(e)
                if !family.is_raw()
                    && matches!(
                        e.raw_os_error(),
                        Some(nix::libc::ECONNREFUSED | nix::libc::EHOSTUNREACH)
                    ) =>
            {
                sock.send_to(&buf, &dst)?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// The stream half of `send_probe()` (probe_unix.c:588-608): open the connecting socket,
    /// and when the source port is unusable take the next sequence number and try again. C
    /// counts the attempts from `MIN_PORT` to `MAX_PORT` exclusive — deliberately one short of
    /// exhausting the range — and so do we. Every failed attempt drops its socket, so a retry
    /// leaks no descriptor.
    fn start_stream(
        table: &mut ProbeTable,
        idx: usize,
        params: &CProbeParams,
    ) -> std::io::Result<()> {
        let mut last_err = None;
        for _ in MIN_PORT..MAX_PORT {
            let probe = &table.probes[idx];
            let opened = stream::open(
                params.protocol,
                params.ip_version,
                probe.sequence,
                probe.local.ip(),
                probe.remote.ip(),
                params,
            );
            match opened {
                Ok((sock, dest)) => {
                    let probe = &mut table.probes[idx];
                    probe.remote = dest;
                    probe.local.set_port(probe.sequence);
                    probe.stream = Some(sock);
                    return Ok(());
                }
                Err(e)
                    if matches!(
                        e.raw_os_error(),
                        Some(nix::libc::EADDRINUSE | nix::libc::EADDRNOTAVAIL)
                    ) =>
                {
                    let next = table.next_sequence();
                    table.probes[idx].sequence = next;
                    last_err = Some(e);
                }
                Err(e) => {
                    // The destination the probe was aimed at, so that an ECONNREFUSED —
                    // which `command.rs` answers with `reply` (probe_unix.c:613-620) —
                    // still names the right address and port.
                    table.probes[idx].remote.set_port(stream::dest_port(params));
                    return Err(e);
                }
            }
        }
        // Out of attempts: C leaves `errno` at the last EADDRINUSE/EADDRNOTAVAIL, which
        // report_packet_error() prints as `address-in-use` / `address-not-available`.
        Err(last_err.unwrap_or_else(|| std::io::Error::from_raw_os_error(nix::libc::EADDRINUSE)))
    }

    /// `receive_replies_from_probe_socket()` for every outstanding stream probe
    /// (probe_unix.c:851-904, 947-953). The indices are collected first and walked in reverse,
    /// because answering a probe `swap_remove`s it and only moves entries that sit *after* the
    /// one being removed.
    fn check_streams(&self, table: &mut ProbeTable, now: Instant, out: &mut Vec<Response>) {
        let stream_idx: Vec<usize> = table.stream_fds().into_iter().map(|(i, _)| i).collect();
        for idx in stream_idx.into_iter().rev() {
            let Some(sock) = table.probes[idx].stream.as_ref() else {
                continue;
            };
            match stream::check(sock) {
                stream::Completion::Pending => {}
                stream::Completion::Reached => {
                    let from = table.probes[idx].remote.ip();
                    self.respond(table, idx, ProbeResult::Reply, from, now, Vec::new(), out);
                }
                stream::Completion::Failed(e) => {
                    // probe_unix.c:900-903: report_packet_error() then free_probe().
                    let p = table.remove(idx);
                    out.push(crate::backend::error_response(p.token, &e));
                }
            }
        }
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
                Err(nix::errno::Errno::EAGAIN) => {
                    // `sk_err` may already have been consumed by another syscall on this socket
                    // (a later probe's `sendto`, see `send_datagram`), leaving the error-queue
                    // entry behind with nothing to announce it. So on a DGRAM socket look at the
                    // queue itself before giving up; an empty queue is `Ok(None)` and ends the
                    // turn. `Unreachable` is the fallback only if the cmsg carries no ICMP
                    // origin, in which case we know nothing better.
                    if family.is_raw() {
                        return Ok(());
                    }
                    // No cmsg means no ICMP code either, so the fallback carries code 0.
                    match errqueue::read_error(
                        sock,
                        &mut err_buf,
                        QueuedError::Unreachable { code: 0 },
                    )? {
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
            QueuedError::Unreachable { code } => deconstruct::IcmpKind::DestUnreach { code },
        }
    }

    /// `handle_error_queue_packet()` (deconstruct_unix.c:127-146). C picks the branch from the send socket's `SO_PROTOCOL`
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
            // Deviation 26(a): the datagram we sent started with the UDP header we built
            // (construct_udp{4,6}_packet), and the error queue hands that payload back — but
            // with no inner IP header, hence `addrs: None`. The matching rules themselves live
            // once, in `deconstruct::match_udp` (Task 9).
            if payload.len() < UDP_HEADER {
                return;
            }
            let (src_port, dst_port, checksum) = (
                u16::from_be_bytes([payload[0], payload[1]]),
                u16::from_be_bytes([payload[2], payload[3]]),
                u16::from_be_bytes([payload[6], payload[7]]),
            );
            let Some(idx) = deconstruct::match_udp(table, src_port, dst_port, checksum, None)
            else {
                return;
            };
            let result = match Self::queued_icmp_kind(kind, version) {
                deconstruct::IcmpKind::TimeExceeded => ProbeResult::TtlExpired,
                deconstruct::IcmpKind::DestUnreach { code }
                    if code == deconstruct::port_unreach_code(version) =>
                {
                    ProbeResult::Reply
                }
                _ => ProbeResult::NoRouteHost,
            };
            self.respond(table, idx, result, offender, now, Vec::new(), out);
            return;
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
        // `command.rs` sets this too; doing it here as well keeps the backend's own invariant
        // ("the probe records what we sent") true for callers that drive it directly, which
        // `match_udp`'s `protocol == Udp` check depends on.
        probe.protocol = params.protocol;
        probe.departure = Instant::now();
        match params.protocol {
            Protocol::Icmp | Protocol::Udp => self.send_datagram(family, probe, params),
            Protocol::Sctp if !self.sctp => {
                // No SCTP in this kernel: EINVAL → `invalid-argument`, matching C's refusal in
                // `is_protocol_supported()` (probe_unix.c:506-528) rather than leaking the
                // `EPROTONOSUPPORT` that `socket(2)` would otherwise report. The dispatcher
                // already rejects this earlier via `Helper::send_probe`'s `protocol_supported`
                // check; this is the belt-and-braces path for a backend used directly.
                Err(std::io::Error::from_raw_os_error(nix::libc::EINVAL))
            }
            Protocol::Tcp | Protocol::Sctp => Self::start_stream(table, idx, params),
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
        // `ProbeBackend::receive` cannot report failure by return value, and C exits from
        // inside `receive_replies()`: an unexpected receive errno is
        // `error(EXIT_FAILURE, …, "Failure receiving replies")` (probe_unix.c:790). We park it
        // for `take_fatal()` so `serve()` can flush the replies produced by this same call —
        // C's own `printf`s are already out by then — and exit 1 through `Fatal`.
        let mut fatal = None;
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
                fatal = Some(Fatal::Io("Failure receiving replies".into(), e));
                break;
            }
        }
        // C never reaches its probe-socket loop after a fatal receive error: `error(EXIT_FAILURE)`
        // ends the process inside `receive_replies()` (probe_unix.c:790, 947-953).
        if fatal.is_none() {
            self.check_streams(table, now, out);
        }
        self.fatal = self.fatal.take().or(fatal);
    }
    fn take_fatal(&mut self) -> Option<Fatal> {
        self.fatal.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtr_proto::{CProbeParams, ProbeResult, Protocol, ResponseKind};
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
        // Occupy local:seq so the first attempt's bind() fails with EADDRINUSE. If some other
        // process already holds it, so much the better — the retry is what is under test.
        let blocker = std::net::TcpListener::bind(("127.0.0.1", seq)).ok();
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
}
