//! Unix backend (Linux, FreeBSD and macOS): raw sockets, plus the unprivileged DGRAM fallback
//! on Linux. Ported from packet/probe_unix.c (mtr 0.96, commit 7b01773). GPL-2.0-only.
//!
//! What differs between the three is confined to `sockets.rs` (which sockets can be opened and
//! which options exist), `errqueue.rs` (Linux only) and `privs.rs`; the packet construction,
//! reply parsing, matching and the stream probes are the same code on all of them. macOS hands
//! raw IPv4 receivers the `ip_len` field in host byte order (C's `check_length_order()` dance,
//! probe_unix.c:124-190); the parser never reads that field, only the IHL, so nothing here
//! depends on it.
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
mod datagram;
pub mod deconstruct;
#[cfg(target_os = "linux")]
pub mod errqueue;
pub mod sockets;
pub mod stream;
#[cfg(test)]
mod tests;

use std::net::{IpAddr, SocketAddr};
use std::os::fd::BorrowedFd;
use std::time::Instant;

use mtr_proto::{CProbeParams, MplsLabel, ProbeResult, Protocol, Response, ResponseKind};

use super::ProbeBackend;
use crate::Fatal;
use crate::probe_table::{MAX_PORT, MIN_PORT, ProbeTable, addr, rtt_us};
#[cfg(test)]
use deconstruct::parse_icmp4;
use deconstruct::{Parsed, match_reply};
use sockets::Family;

pub struct UnixBackend {
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

impl UnixBackend {
    /// `init_net_state_privileged()` (probe_unix.c:422-459).
    pub fn open_privileged() -> Result<Self, Fatal> {
        let v4 = Family::open(4);
        let v6 = Family::open(6);
        if v4.is_err() && v6.is_err() {
            let e4 = v4.err().map(|e| e.to_string()).unwrap_or_default();
            let e6 = v6.err().map(|e| e.to_string()).unwrap_or_default();
            return Err(Fatal::Message(format!(
                "Failure to open IPv4 sockets: {e4}\n{}: Failure to open IPv6 sockets: {e6}",
                crate::PROGRAM
            )));
        }
        Ok(UnixBackend {
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
}

impl ProbeBackend for UnixBackend {
    fn ip_version_supported(&self, version: u8) -> bool {
        self.family(version).is_some()
    }
    fn protocol_supported(&self, protocol: Protocol) -> bool {
        protocol != Protocol::Sctp || self.sctp
    }
    /// Deviation 34: `mark` is answered honestly. C says `ok` whenever `SO_MARK` compiles and
    /// then fails in `setsockopt()` after the privilege drop; we report what the kernel will
    /// actually accept, which is `CAP_NET_ADMIN` surviving `privs::drop_all()`.
    fn mark_supported(&self) -> bool {
        crate::privs::has_net_admin()
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
                #[cfg(target_os = "linux")]
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
