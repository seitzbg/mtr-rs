//! Outstanding probes, sequence numbers and timeouts. Ported from packet/probe.c:44-205,
//! 333-404 and packet/probe_unix.c:642-652, 989-1053 (mtr 0.96, commit 7b01773). GPL-2.0-only.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::fd::{AsFd, BorrowedFd};
use std::time::{Duration, Instant};

use mtr_proto::{Protocol, Response, ResponseKind};

/// `MAX_PROBES` (probe.h:37).
pub const MAX_PROBES: usize = 10_240;
/// `MIN_PORT`/`MAX_PORT` (probe_unix.h:28-29): sequence numbers double as source ports.
pub const MIN_PORT: u16 = 33434;
pub const MAX_PORT: u16 = 65535;

/// `probe_t` + `probe_platform_t` (probe.h:91-116, probe_unix.h:32-41).
pub struct Probe {
    pub token: i32,
    pub sequence: u16,
    pub protocol: Protocol,
    pub remote: SocketAddr,
    pub local: SocketAddr,
    pub departure: Instant,
    pub timeout_at: Instant,
    /// The connect()ing socket of a TCP/SCTP probe (`probe_platform_t.socket`).
    pub stream: Option<socket2::Socket>,
}

/// `net_state_t.outstanding_probes` + `next_sequence`.
pub struct ProbeTable {
    pub probes: Vec<Probe>,
    next_sequence: u16,
}

impl Default for ProbeTable {
    fn default() -> Self {
        Self::new()
    }
}

impl ProbeTable {
    pub fn new() -> Self {
        ProbeTable {
            probes: Vec::new(),
            next_sequence: MIN_PORT,
        }
    }

    pub fn len(&self) -> usize {
        self.probes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.probes.is_empty()
    }

    /// `platform_alloc_probe()` (probe_unix.c:643-652): hand out the current value, then
    /// advance and wrap past `MAX_PORT`.
    pub fn next_sequence(&mut self) -> u16 {
        let s = self.next_sequence;
        self.next_sequence = if s == MAX_PORT { MIN_PORT } else { s + 1 };
        s
    }

    /// `alloc_probe()` (probe.c:132-157) with the departure/timeout stamps `send_probe()` sets
    /// (probe_unix.c:581, 638-639). A non-positive timeout expires at the next check.
    pub fn alloc(&mut self, token: i32, now: Instant, timeout_s: i32) -> Option<usize> {
        if self.probes.len() >= MAX_PROBES {
            return None;
        }
        let sequence = self.next_sequence();
        let timeout_at = now + Duration::from_secs(u64::try_from(timeout_s).unwrap_or(0));
        let unspecified = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);
        self.probes.push(Probe {
            token,
            sequence,
            protocol: Protocol::Icmp,
            remote: unspecified,
            local: unspecified,
            departure: now,
            timeout_at,
            stream: None,
        });
        Some(self.probes.len() - 1)
    }

    /// `free_probe()`; closing the stream socket happens on drop.
    pub fn remove(&mut self, idx: usize) -> Probe {
        self.probes.swap_remove(idx)
    }

    /// `find_probe()` (probe.c:176-205) minus the ICMP id check, which the caller does.
    pub fn find_by_sequence(&self, sequence: u16) -> Option<usize> {
        self.probes.iter().position(|p| p.sequence == sequence)
    }

    /// `check_probe_timeouts()` (probe_unix.c:989-1010): strictly past the deadline → `no-reply`.
    pub fn expire(&mut self, now: Instant, out: &mut Vec<Response>) {
        let mut i = 0;
        while i < self.probes.len() {
            if self.probes[i].timeout_at < now {
                let p = self.probes.swap_remove(i);
                out.push(Response {
                    token: p.token,
                    kind: ResponseKind::NoReply,
                });
            } else {
                i += 1;
            }
        }
    }

    /// `get_next_probe_timeout()` (probe_unix.c:1020-1053), clamped at zero.
    pub fn next_timeout(&self, now: Instant) -> Option<Duration> {
        self.probes
            .iter()
            .map(|p| p.timeout_at.saturating_duration_since(now))
            .min()
    }

    /// `gather_probe_sockets()` (probe_unix.c:960-982).
    pub fn stream_fds(&self) -> Vec<(usize, BorrowedFd<'_>)> {
        self.probes
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.stream.as_ref().map(|s| (i, s.as_fd())))
            .collect()
    }
}

/// `receive_probe()`'s `round_trip_us` (probe_unix.c:692-694), monotonic and saturated
/// (deviation 28).
pub fn rtt_us(departure: Instant, now: Instant) -> u32 {
    u32::try_from(now.saturating_duration_since(departure).as_micros()).unwrap_or(u32::MAX)
}

pub mod addr {
    use super::*;

    /// `decode_address_string()` (probe.c:44-89): `inet_pton` for the requested family only.
    pub fn decode(ip_version: u8, s: &str) -> Option<IpAddr> {
        match ip_version {
            4 => s.parse::<Ipv4Addr>().ok().map(IpAddr::V4),
            6 => s.parse::<Ipv6Addr>().ok().map(IpAddr::V6),
            _ => None,
        }
    }

    /// `find_source_addr()` (probe.c:333-404): connect a UDP socket to `dest:1` and read the
    /// local address back; Linux tolerates `EHOSTUNREACH` by falling back to the unspecified
    /// address so an unreachable target can still be probed.
    pub fn find_source_addr(dest: IpAddr) -> std::io::Result<IpAddr> {
        let unspecified: IpAddr = if dest.is_ipv6() {
            Ipv6Addr::UNSPECIFIED.into()
        } else {
            Ipv4Addr::UNSPECIFIED.into()
        };
        let sock = std::net::UdpSocket::bind(SocketAddr::new(unspecified, 0))?;
        match sock.connect(SocketAddr::new(dest, 1)) {
            Ok(()) => Ok(sock.local_addr()?.ip()),
            Err(e) if e.raw_os_error() == Some(nix::libc::EHOSTUNREACH) => Ok(unspecified),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn sequences_start_at_min_port_and_wrap() {
        let mut t = ProbeTable::new();
        assert_eq!(t.next_sequence(), MIN_PORT);
        assert_eq!(t.next_sequence(), MIN_PORT + 1);
        t.next_sequence = MAX_PORT;
        assert_eq!(t.next_sequence(), MAX_PORT);
        assert_eq!(t.next_sequence(), MIN_PORT);
    }

    #[test]
    fn alloc_assigns_sequence_and_timeout_and_refuses_past_max_probes() {
        let mut t = ProbeTable::new();
        let now = t0();
        let i = t.alloc(42, now, 3).unwrap();
        assert_eq!(t.probes[i].token, 42);
        assert_eq!(t.probes[i].sequence, MIN_PORT);
        assert_eq!(t.probes[i].timeout_at, now + Duration::from_secs(3));
        assert_eq!(t.find_by_sequence(MIN_PORT), Some(i));
        for k in 1..MAX_PROBES {
            assert!(t.alloc(k as i32, now, 60).is_some());
        }
        assert_eq!(t.len(), MAX_PROBES);
        assert!(t.alloc(1, now, 60).is_none(), "probes-exhausted");
    }

    #[test]
    fn negative_or_zero_timeout_expires_on_the_next_check() {
        let mut t = ProbeTable::new();
        let now = t0();
        t.alloc(1, now, 0).unwrap();
        t.alloc(2, now, -5).unwrap();
        t.alloc(3, now, 10).unwrap();
        assert_eq!(t.next_timeout(now), Some(Duration::ZERO));
        let mut out = Vec::new();
        t.expire(now + Duration::from_millis(1), &mut out);
        let mut tokens: Vec<i32> = out.iter().map(|r| r.token).collect();
        tokens.sort_unstable();
        assert_eq!(tokens, vec![1, 2]);
        assert!(
            out.iter()
                .all(|r| r.kind == mtr_proto::ResponseKind::NoReply)
        );
        assert_eq!(t.len(), 1);
        assert_eq!(t.next_timeout(now), Some(Duration::from_secs(10)));
        assert_eq!(ProbeTable::new().next_timeout(now), None);
    }

    #[test]
    fn expiry_is_strict_like_compare_timeval() {
        let mut t = ProbeTable::new();
        let now = t0();
        t.alloc(1, now, 1).unwrap();
        let mut out = Vec::new();
        t.expire(now + Duration::from_secs(1), &mut out); // equal → not yet
        assert!(out.is_empty());
        t.expire(
            now + Duration::from_secs(1) + Duration::from_nanos(1),
            &mut out,
        );
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn rtt_is_microseconds_saturated() {
        let now = t0();
        assert_eq!(rtt_us(now, now + Duration::from_micros(1234)), 1234);
        assert_eq!(rtt_us(now, now + Duration::from_secs(1 << 40)), u32::MAX);
        assert_eq!(rtt_us(now + Duration::from_secs(1), now), 0);
    }

    #[test]
    fn addresses_decode_per_ip_version_and_source_is_found() {
        assert_eq!(
            addr::decode(4, "127.0.0.1"),
            Some("127.0.0.1".parse().unwrap())
        );
        assert_eq!(addr::decode(6, "::1"), Some("::1".parse().unwrap()));
        assert_eq!(addr::decode(4, "::1"), None);
        assert_eq!(addr::decode(6, "127.0.0.1"), None);
        assert_eq!(addr::decode(0, "127.0.0.1"), None);
        assert_eq!(addr::decode(4, "str-value"), None);
        // Loopback always has a route; the source is loopback too.
        assert_eq!(
            addr::find_source_addr("127.0.0.1".parse().unwrap()).unwrap(),
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );
    }
}
