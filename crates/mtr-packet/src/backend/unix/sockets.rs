//! Socket opening, fallback and per-probe options. Ported from packet/probe_unix.c:209-486
//! and packet/construct_unix.c:297-406, 614-698, 766-826 (mtr 0.96, commit 7b01773).
//! GPL-2.0-only.
//!
//! The raw sockets are the same on Linux, FreeBSD and macOS. The unprivileged fallback is Linux
//! only: it needs `IP_RECVERR` to hear time-exceeded on a "ping" socket, which neither BSD has,
//! so there a helper that cannot open raw sockets fails to start, exactly as C's does without
//! `HAVE_LINUX_ERRQUEUE_H`.

use std::net::SocketAddr;
use std::os::fd::{AsFd, BorrowedFd};

use mtr_proto::CProbeParams;
#[cfg(target_os = "linux")]
use nix::sys::socket::{setsockopt, sockopt};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};

pub enum Sockets {
    /// `open_ip{4,6}_sockets_raw()`: send ICMP, send UDP, receive ICMP.
    Raw {
        icmp_send: Socket,
        udp_send: Socket,
        recv: Socket,
    },
    /// `open_ip{4,6}_sockets_dgram()`: unprivileged ICMP and UDP sockets with `IP_RECVERR`.
    #[cfg(target_os = "linux")]
    Dgram { icmp: Socket, udp: Socket },
}

pub struct Family {
    pub version: u8,
    pub sockets: Sockets,
}

/// `SO_SNDBUF` for the raw send sockets: comfortably above `PACKET_BUFFER_SIZE` plus headers.
const SEND_BUFFER_SIZE: usize = 65536;
/// `SO_RCVBUF` for the raw receive socket: many maximum-size replies.
const RECV_BUFFER_SIZE: usize = 262_144;

fn domain(version: u8) -> Domain {
    if version == 6 {
        Domain::IPV6
    } else {
        Domain::IPV4
    }
}

fn icmp_protocol(version: u8) -> Protocol {
    if version == 6 {
        Protocol::ICMPV6
    } else {
        Protocol::ICMPV4
    }
}

#[cfg(target_os = "linux")]
fn enable_recverr(sock: &Socket, version: u8) -> std::io::Result<()> {
    let r = if version == 6 {
        setsockopt(sock, sockopt::Ipv6RecvErr, &true)
    } else {
        setsockopt(sock, sockopt::Ipv4RecvErr, &true)
    };
    r.map_err(std::io::Error::from)
}

impl Family {
    /// Raw first; on any failure the DGRAM fallback (probe_unix.c:432-447) on Linux, and the
    /// raw sockets' own error everywhere else.
    pub fn open(version: u8) -> std::io::Result<Family> {
        let raw = (|| -> std::io::Result<Sockets> {
            let icmp_send = Socket::new(domain(version), Type::RAW, Some(icmp_protocol(version)))?;
            let udp_send = Socket::new(domain(version), Type::RAW, Some(Protocol::UDP))?;
            // A probe may be PACKET_BUFFER_SIZE (9000) bytes. Darwin's raw-socket send buffer
            // defaults to 8192 (`rip_sendspace`), which turns such a `sendto` into `EMSGSIZE`;
            // FreeBSD's 9216 barely fits and Linux's is far larger. Ask for room everywhere.
            for sock in [&icmp_send, &udp_send] {
                sock.set_send_buffer_size(SEND_BUFFER_SIZE)?;
            }
            let recv = Socket::new(domain(version), Type::RAW, Some(icmp_protocol(version)))?;
            // Same story on receive: Darwin's `rip_recvspace` is 8192, so the 9020-byte reply to
            // a maximum-size echo is dropped by `sbappendaddr()` before `recvfrom` can see it.
            // A larger queue also rides out reply bursts on a busy machine.
            recv.set_recv_buffer_size(RECV_BUFFER_SIZE)?;
            Ok(Sockets::Raw {
                icmp_send,
                udp_send,
                recv,
            })
        })();
        let sockets = match raw {
            Ok(s) => s,
            #[cfg(not(target_os = "linux"))]
            Err(e) => return Err(e),
            #[cfg(target_os = "linux")]
            Err(_) => {
                let icmp = Socket::new(domain(version), Type::DGRAM, Some(icmp_protocol(version)))?;
                enable_recverr(&icmp, version)?;
                let udp = Socket::new(domain(version), Type::DGRAM, Some(Protocol::UDP))?;
                enable_recverr(&udp, version)?;
                Sockets::Dgram { icmp, udp }
            }
        };
        Ok(Family { version, sockets })
    }

    pub fn is_raw(&self) -> bool {
        match self.sockets {
            Sockets::Raw { .. } => true,
            #[cfg(target_os = "linux")]
            Sockets::Dgram { .. } => false,
        }
    }

    /// `init_net_state()`: only the sockets we read from go non-blocking.
    pub fn set_nonblocking(&self) -> std::io::Result<()> {
        for fd_sock in self.recv_sockets() {
            fd_sock.set_nonblocking(true)?;
        }
        Ok(())
    }

    fn recv_sockets(&self) -> Vec<&Socket> {
        match &self.sockets {
            Sockets::Raw { recv, .. } => vec![recv],
            #[cfg(target_os = "linux")]
            Sockets::Dgram { icmp, udp } => vec![icmp, udp],
        }
    }

    pub fn recv_fds(&self) -> Vec<BorrowedFd<'_>> {
        self.recv_sockets().into_iter().map(AsFd::as_fd).collect()
    }

    pub fn icmp_send(&self) -> &Socket {
        match &self.sockets {
            Sockets::Raw { icmp_send, .. } => icmp_send,
            #[cfg(target_os = "linux")]
            Sockets::Dgram { icmp, .. } => icmp,
        }
    }

    pub fn udp_send(&self) -> &Socket {
        match &self.sockets {
            Sockets::Raw { udp_send, .. } => udp_send,
            #[cfg(target_os = "linux")]
            Sockets::Dgram { udp, .. } => udp,
        }
    }
}

/// `IPPROTO_SCTP` by its IANA number: socket2 only names it on platforms whose libc does, and
/// macOS's does not. The `socket()` call is the support probe, so an unknown protocol number is
/// exactly the `EPROTONOSUPPORT` we want there.
pub fn sctp_protocol() -> Protocol {
    Protocol::from(132)
}

/// `check_sctp_support()` (probe_unix.c:209-222).
pub fn check_sctp_support() -> bool {
    Socket::new(Domain::IPV4, Type::STREAM, Some(sctp_protocol())).is_ok()
}

/// The first half of C's option block: the routing mark and the bind-to-device, both applied
/// *before* the bind (construct_unix.c:624-636 for v4, 766-778 for v6).
#[cfg(target_os = "linux")]
pub fn set_mark_and_device(sock: &Socket, params: &CProbeParams) -> std::io::Result<()> {
    if params.routing_mark != 0 {
        // Needs CAP_NET_ADMIN, so only touched when the client asked for it.
        sock.set_mark(params.routing_mark)?;
    }
    if let Some(dev) = &params.local_device {
        sock.bind_device(Some(dev.as_bytes()))?;
    }
    Ok(())
}

/// The BSDs have no `SO_MARK`, and only macOS has a per-socket interface bind (`IP_BOUND_IF` /
/// `IPV6_BOUND_IF`, by index). C compiles both blocks out (`#ifdef SO_MARK` /
/// `#ifdef SO_BINDTODEVICE`, construct_unix.c:624-638) and so silently probes without them; a
/// request that asked for something the platform cannot do is answered with `invalid-argument`
/// here instead, since a probe sent through the wrong table or interface is not the probe that
/// was asked for. The client never sends `mark` where `check-support` said no, and resolves
/// `-I` to a `local-ip` itself, so in practice neither reaches a BSD helper.
#[cfg(not(target_os = "linux"))]
pub fn set_mark_and_device(sock: &Socket, params: &CProbeParams) -> std::io::Result<()> {
    let einval = || std::io::Error::from_raw_os_error(nix::libc::EINVAL);
    if params.routing_mark != 0 {
        return Err(einval());
    }
    if let Some(dev) = &params.local_device {
        #[cfg(target_os = "macos")]
        {
            // An unknown name is `ENODEV`, as `SO_BINDTODEVICE` reports it on Linux.
            let index = nix::net::if_::if_nametoindex(dev.as_str())
                .map_err(|_| std::io::Error::from_raw_os_error(nix::libc::ENODEV))?;
            let index = std::num::NonZeroU32::new(index).ok_or_else(einval)?;
            if params.ip_version == 6 {
                sock.bind_device_by_index_v6(Some(index))?;
            } else {
                sock.bind_device_by_index_v4(Some(index))?;
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (sock, dev);
            return Err(einval());
        }
    }
    Ok(())
}

/// The second half: TOS and TTL, which C sets *after* the bind (construct_unix.c:679-698,
/// 812-826).
///
/// A TTL outside 0..=255 is `EINVAL` here rather than left to the kernel: Linux rejects it in
/// `IP_TTL` and both kernels in `IPV6_UNICAST_HOPS`, but FreeBSD's `IP_TTL` stores the low byte
/// without complaint, which would turn `ttl 300` into a silent 44-hop probe.
pub fn set_tos_and_ttl(sock: &Socket, version: u8, params: &CProbeParams) -> std::io::Result<()> {
    let einval = || std::io::Error::from_raw_os_error(nix::libc::EINVAL);
    let tos = u32::try_from(params.type_of_service).map_err(|_| einval())?;
    let ttl = u32::try_from(params.ttl)
        .ok()
        .filter(|t| *t <= 255)
        .ok_or_else(einval)?;
    if version == 6 {
        sock.set_tclass_v6(tos)?;
        sock.set_unicast_hops_v6(ttl)?;
    } else {
        sock.set_tos_v4(tos)?;
        sock.set_ttl_v4(ttl)?;
    }
    Ok(())
}

/// The whole mark/device/TOS/TTL block for a socket that binds itself: the stream probes
/// (construct_unix.c:324-406) bind inside `open_stream_socket()`, so they apply both halves at
/// once. Keeping it in one place is why `stream.rs` (Task 13) has no copy of it.
pub fn set_common_options(
    sock: &Socket,
    version: u8,
    params: &CProbeParams,
) -> std::io::Result<()> {
    set_mark_and_device(sock, params)?;
    set_tos_and_ttl(sock, version, params)
}

/// Per-probe options on a shared send socket, in C's order: mark, device, bind, TOS, TTL
/// (construct_unix.c:624-698). `local: None` means "do not bind" — the stream path binds
/// itself to `local:sequence` after setting `SO_REUSEPORT`/`SO_REUSEADDR` (Task 13).
///
/// A raw socket is bound to the address alone. The UDP path records its source port in
/// `local` for reply matching, but a raw socket has no port: Linux ignores one in `bind()`,
/// and FreeBSD compares the whole `sockaddr` against the interface addresses
/// (`ifa_ifwithaddr()`, `rip_bind()`) and rejects a non-zero port with `EADDRNOTAVAIL`.
pub fn apply_probe_options(
    sock: &Socket,
    version: u8,
    params: &CProbeParams,
    local: Option<SocketAddr>,
    is_raw: bool,
) -> std::io::Result<()> {
    set_mark_and_device(sock, params)?;
    if let Some(mut local) = local {
        if is_raw {
            local.set_port(0);
        }
        let already_bound = match sock.local_addr()?.as_socket() {
            Some(cur) if is_raw => cur.ip() == local.ip(),
            Some(cur) => cur.port() != 0,
            None => false,
        };
        if !already_bound {
            sock.bind(&SockAddr::from(local))?;
        }
    }
    set_tos_and_ttl(sock, version, params)
}

/// Whether a probe socket for `version` can be opened at all. The loopback tests gate on this:
/// with neither `cap_net_raw` nor open ping sockets (Linux), or without root (the BSDs), there is
/// no socket to probe *with*, and those tests return early instead of failing (Global
/// Constraints). Deliberately not `#[cfg(test)]`, so integration tests can gate on it too — and
/// deliberately *not* used by `ipv4_opens_raw_or_falls_back_to_dgram_with_recverr` below, which
/// must fail rather than pass vacuously if opening ever regresses.
pub fn probe_sockets_available(version: u8) -> bool {
    Family::open(version).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use nix::sys::socket::getsockopt;

    /// Set (any non-empty value, e.g. in CI simulation) to force the two "opens on this box"
    /// tests below to take the skip path even when the box actually does allow ping/raw
    /// sockets — used to rehearse the GitHub-runner environment locally.
    const FORCE_NO_PING_ENV: &str = "MTR_TEST_FORCE_NO_PING";

    fn env_forces_no_ping() -> bool {
        std::env::var_os(FORCE_NO_PING_ENV).is_some()
    }

    /// Parses `/proc/sys/net/ipv4/ping_group_range`'s `"lo hi"` (tab- or space-separated).
    #[cfg(target_os = "linux")]
    fn parse_ping_group_range(s: &str) -> Option<(u32, u32)> {
        let mut it = s.split_whitespace();
        let lo: u32 = it.next()?.parse().ok()?;
        let hi: u32 = it.next()?.parse().ok()?;
        Some((lo, hi))
    }

    /// A range with `lo > hi` (the kernel default, "1 0") allows no gid at all.
    #[cfg(target_os = "linux")]
    fn range_allows_gid(range: (u32, u32), gid: u32) -> bool {
        range.0 <= range.1 && gid >= range.0 && gid <= range.1
    }

    /// Independent env signal #1: does this process belong to a gid inside
    /// `ping_group_range`? That's what lets an unprivileged process open a DGRAM ICMP
    /// ("ping") socket at all, raw or otherwise.
    #[cfg(target_os = "linux")]
    fn ping_sockets_allowed() -> bool {
        if env_forces_no_ping() {
            return false;
        }
        let Ok(contents) = std::fs::read_to_string("/proc/sys/net/ipv4/ping_group_range") else {
            return false;
        };
        let Some(range) = parse_ping_group_range(&contents) else {
            return false;
        };
        if range_allows_gid(range, nix::unistd::getgid().as_raw()) {
            return true;
        }
        // `getgroups()` needs nix's "user" feature, which this crate enables; fall back to
        // egid-only if that ever changes.
        nix::unistd::getgroups()
            .map(|groups| groups.iter().any(|g| range_allows_gid(range, g.as_raw())))
            .unwrap_or(false)
    }

    /// No `IP_RECVERR` on the BSDs, hence no fallback: without root there is no probe socket.
    #[cfg(not(target_os = "linux"))]
    fn ping_sockets_allowed() -> bool {
        false
    }

    /// Independent env signal #2: CAP_NET_RAW in the effective capability set, per
    /// `/proc/self/status`'s `CapEff` (bit 13 — see capability.h).
    #[cfg(target_os = "linux")]
    fn raw_sockets_allowed() -> bool {
        if env_forces_no_ping() {
            return false;
        }
        let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
            return false;
        };
        for line in status.lines() {
            if let Some(hex) = line.strip_prefix("CapEff:")
                && let Ok(val) = u64::from_str_radix(hex.trim(), 16)
            {
                return val & (1 << 13) != 0;
            }
        }
        false
    }

    /// FreeBSD gates raw sockets on `PRIV_NETINET_RAW` and macOS on euid 0: root either way.
    #[cfg(not(target_os = "linux"))]
    fn raw_sockets_allowed() -> bool {
        !env_forces_no_ping() && nix::unistd::geteuid().is_root()
    }

    /// Whether this host has any IPv6 configured at all (even just loopback); empty/missing
    /// means the kernel has IPv6 fully disabled and `Family::open(6)` has nothing to bind.
    #[cfg(target_os = "linux")]
    fn ipv6_available() -> bool {
        std::fs::read_to_string("/proc/net/if_inet6")
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    }

    /// No `/proc` on the BSDs; a loopback `::1` bind is the equivalent question.
    #[cfg(not(target_os = "linux"))]
    fn ipv6_available() -> bool {
        std::net::UdpSocket::bind("[::1]:0").is_ok()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ping_group_range_parser_gates_by_gid() {
        let disallow_all = parse_ping_group_range("1\t0").unwrap();
        assert!(!range_allows_gid(disallow_all, 0));
        assert!(!range_allows_gid(disallow_all, 1));
        assert!(!range_allows_gid(disallow_all, 1000));

        let allow_all = parse_ping_group_range("0\t2147483647").unwrap();
        assert!(range_allows_gid(allow_all, 0));
        assert!(range_allows_gid(allow_all, 1000));
        assert!(range_allows_gid(allow_all, 2147483647));

        assert_eq!(parse_ping_group_range("garbage"), None);
    }

    /// The DGRAM fallback is worthless without `IP_RECVERR`/`IPV6_RECVERR`: it is the only way
    /// a time-exceeded ever reaches us on a ping socket (probe_unix.c:815-819).
    #[cfg(target_os = "linux")]
    fn assert_recverr(f: &Family) {
        if let Sockets::Dgram { icmp, udp } = &f.sockets {
            for sock in [icmp, udp] {
                let on = if f.version == 6 {
                    getsockopt(sock, sockopt::Ipv6RecvErr).unwrap()
                } else {
                    getsockopt(sock, sockopt::Ipv4RecvErr).unwrap()
                };
                assert!(
                    on,
                    "RECVERR must be set on every IPv{} DGRAM socket",
                    f.version
                );
            }
        }
    }

    /// Gated only on an *independent* environment signal (CAP_NET_RAW or an allowed ping
    /// group), never on `dgram_available()`/`Family::open().is_ok()` — that would make this
    /// test pass vacuously if opening ever regressed (Task 7 review). On a box with neither
    /// signal (e.g. a GitHub-hosted runner with default `ping_group_range` and no
    /// `cap_net_raw`) neither raw nor DGRAM sockets can open, so we skip instead of failing.
    #[test]
    fn ipv4_opens_raw_or_falls_back_to_dgram_with_recverr() {
        if !raw_sockets_allowed() && !ping_sockets_allowed() {
            return;
        }
        let f = Family::open(4).expect("raw sockets with cap_net_raw, else open ping sockets");
        assert_eq!(f.version, 4);
        // Unprivileged test processes get the DGRAM pair; with cap_net_raw the raw triple.
        match &f.sockets {
            #[cfg(target_os = "linux")]
            Sockets::Dgram { icmp, udp } => {
                assert_eq!(icmp.protocol().unwrap(), Some(socket2::Protocol::ICMPV4));
                assert_eq!(udp.protocol().unwrap(), Some(socket2::Protocol::UDP));
                assert_eq!(f.recv_fds().len(), 2);
            }
            Sockets::Raw { .. } => assert_eq!(f.recv_fds().len(), 1),
        }
        #[cfg(target_os = "linux")]
        assert_recverr(&f);
        f.set_nonblocking().unwrap();
    }

    /// Without the Linux fallback an unprivileged open fails outright: that is what makes the
    /// setuid-root install on the BSDs necessary, so pin it rather than let a stray fallback
    /// creep in and hide a broken install as a helper that "works" and hears nothing.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn without_root_there_is_no_probe_socket_at_all() {
        if raw_sockets_allowed() {
            return;
        }
        let e = match Family::open(4) {
            Ok(_) => panic!("raw sockets opened without root"),
            Err(e) => e,
        };
        assert_eq!(e.raw_os_error(), Some(nix::libc::EPERM), "{e}");
        assert!(!probe_sockets_available(4));
    }

    #[test]
    fn ipv6_opens_too_on_this_box() {
        if !raw_sockets_allowed() && !ping_sockets_allowed() {
            return;
        }
        if !ipv6_available() {
            return;
        }
        // The box has link-local + ULA IPv6 but no global address; opening the sockets and
        // reaching `::1` works regardless, which is all this test and the v6 loopback tests need.
        let f = Family::open(6).expect("IPv6 sockets open (loopback IPv6 is always present)");
        assert_eq!(f.version, 6);
        #[cfg(target_os = "linux")]
        assert_recverr(&f);
    }

    /// Whether the kernel has SCTP: `/proc/net/protocols` on Linux (read *after* the probe,
    /// since opening the socket is what autoloads the module), `kldstat -m sctp` on FreeBSD
    /// (which also finds it when it is compiled into the kernel), never on macOS.
    fn sctp_in_kernel() -> bool {
        #[cfg(target_os = "linux")]
        {
            std::fs::read_to_string("/proc/net/protocols")
                .map(|s| s.lines().any(|l| l.starts_with("SCTP")))
                .unwrap_or(false)
        }
        #[cfg(target_os = "macos")]
        {
            false
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            std::process::Command::new("kldstat")
                .args(["-q", "-m", "sctp"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    }

    #[test]
    fn sctp_support_is_detected() {
        let detected = check_sctp_support();
        let module_present = sctp_in_kernel();
        if module_present {
            assert!(
                detected,
                "the sctp module is loaded, so the probe socket must open"
            );
        } else {
            assert!(
                !detected,
                "no SCTP in /proc/net/protocols, yet a socket opened"
            );
        }
    }

    #[test]
    fn probe_options_apply_to_a_probe_socket() {
        if !probe_sockets_available(4) {
            eprintln!("skipping: no IPv4 probe sockets");
            return;
        }
        let f = Family::open(4).unwrap();
        let params = mtr_proto::CProbeParams {
            ttl: 7,
            type_of_service: 0x10,
            ..Default::default()
        };
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        apply_probe_options(f.icmp_send(), 4, &params, Some(local), f.is_raw()).unwrap();
        assert_eq!(f.icmp_send().ttl_v4().unwrap(), 7);
        assert_eq!(f.icmp_send().tos_v4().unwrap(), 0x10);
        // With `local: None` (the stream path, Task 13) nothing is bound.
        let unbound = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
        apply_probe_options(&unbound, 4, &params, None, false).unwrap();
        assert_eq!(unbound.ttl_v4().unwrap(), 7);
        assert_eq!(unbound.local_addr().unwrap().as_socket().unwrap().port(), 0);
    }
}
