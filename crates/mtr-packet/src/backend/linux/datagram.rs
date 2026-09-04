//! ICMP/UDP datagram send and receive for the Linux backend: raw sockets and the DGRAM +
//! IP_RECVERR fallback. Split out of mod.rs; ported from packet/probe_unix.c (mtr 0.96,
//! commit 7b01773). GPL-2.0-only.

use std::net::IpAddr;
use std::os::fd::AsRawFd;
use std::time::Instant;

use mtr_proto::{CProbeParams, ProbeResult, Protocol, Response};
use socket2::{SockAddr, Socket};

use super::LinuxBackend;
use super::construct::{
    self, PACKET_BUFFER_SIZE, UDP_HEADER, icmp_echo, packet_size, udp_datagram, udp_ports,
    udp_source_port_from_pid,
};
use super::deconstruct::{self, Inner, Parsed, parse_icmp4, parse_icmp6};
use super::errqueue::{self, QueuedError};
use super::sockets::{Family, apply_probe_options};
use crate::probe_table::{Probe, ProbeTable};

impl LinuxBackend {
    /// `construct_packet()` + `send_packet()` for ICMP and UDP (construct_unix.c:832-872,
    /// probe_unix.c:47-123). The buffer starts out filled with the bit pattern
    /// (construct_unix.c:855) and each builder overwrites only its own header. Stream
    /// protocols never reach here; they go to `stream.rs` (Task 13).
    pub(super) fn send_datagram(
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

    /// `receive_replies_from_recv_socket()` for one socket (probe_unix.c:704-844). `recvfrom`
    /// fills a plain `&mut [u8]`, so no `MaybeUninit`/`unsafe` is needed. Every errno other
    /// than `EAGAIN`, `EINTR`, `EHOSTUNREACH` and `ECONNREFUSED` is returned to the caller: C
    /// calls `error(EXIT_FAILURE, errno, "Failure receiving replies")` there
    /// (probe_unix.c:790), and so must we — a silently swallowed receive error would look like
    /// a network of timeouts.
    pub(super) fn drain_socket(
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

    /// `handle_error_queue_packet()` (deconstruct_unix.c:127-146). C picks the branch from the send socket's `SO_PROTOCOL`
    /// (probe_unix.c:824-828); `Socket::protocol()` is the same `getsockopt`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn deliver_queued(
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
            let result = match queued_icmp_kind(kind, version) {
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
            kind: queued_icmp_kind(kind, version),
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
