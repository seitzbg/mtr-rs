//! Socket opening, fallback and per-probe options. Ported from packet/probe_unix.c:209-486
//! and packet/construct_unix.c:297-406, 614-698, 766-826 (mtr 0.96, commit 7b01773).
//! GPL-2.0-only.

use std::net::SocketAddr;
use std::os::fd::{AsFd, BorrowedFd};

use mtr_proto::CProbeParams;
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
    Dgram { icmp: Socket, udp: Socket },
}

pub struct Family {
    pub version: u8,
    pub sockets: Sockets,
}

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

fn enable_recverr(sock: &Socket, version: u8) -> std::io::Result<()> {
    let r = if version == 6 {
        setsockopt(sock, sockopt::Ipv6RecvErr, &true)
    } else {
        setsockopt(sock, sockopt::Ipv4RecvErr, &true)
    };
    r.map_err(std::io::Error::from)
}

impl Family {
    /// Raw first; on any failure the DGRAM fallback (probe_unix.c:432-447).
    pub fn open(version: u8) -> std::io::Result<Family> {
        let raw = (|| -> std::io::Result<Sockets> {
            let icmp_send = Socket::new(domain(version), Type::RAW, Some(icmp_protocol(version)))?;
            let udp_send = Socket::new(domain(version), Type::RAW, Some(Protocol::UDP))?;
            let recv = Socket::new(domain(version), Type::RAW, Some(icmp_protocol(version)))?;
            Ok(Sockets::Raw {
                icmp_send,
                udp_send,
                recv,
            })
        })();
        let sockets = match raw {
            Ok(s) => s,
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
        matches!(self.sockets, Sockets::Raw { .. })
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
            Sockets::Dgram { icmp, udp } => vec![icmp, udp],
        }
    }

    pub fn recv_fds(&self) -> Vec<BorrowedFd<'_>> {
        self.recv_sockets().into_iter().map(AsFd::as_fd).collect()
    }

    pub fn icmp_send(&self) -> &Socket {
        match &self.sockets {
            Sockets::Raw { icmp_send, .. } => icmp_send,
            Sockets::Dgram { icmp, .. } => icmp,
        }
    }

    pub fn udp_send(&self) -> &Socket {
        match &self.sockets {
            Sockets::Raw { udp_send, .. } => udp_send,
            Sockets::Dgram { udp, .. } => udp,
        }
    }
}

/// `check_sctp_support()` (probe_unix.c:209-222).
pub fn check_sctp_support() -> bool {
    Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::SCTP)).is_ok()
}

/// The mark/device/TOS/TTL block that both the datagram probes (construct_unix.c:624-698 for
/// v4, 766-826 for v6) and the stream probes (construct_unix.c:324-406) apply. Keeping it in
/// one place is why `stream.rs` (Task 13) has no copy of it.
pub fn set_common_options(
    sock: &Socket,
    version: u8,
    params: &CProbeParams,
) -> std::io::Result<()> {
    let einval = || std::io::Error::from_raw_os_error(nix::libc::EINVAL);
    if params.routing_mark != 0 {
        // Needs CAP_NET_ADMIN, so only touched when the client asked for it.
        sock.set_mark(params.routing_mark)?;
    }
    if let Some(dev) = &params.local_device {
        sock.bind_device(Some(dev.as_bytes()))?;
    }
    let tos = u32::try_from(params.type_of_service).map_err(|_| einval())?;
    let ttl = u32::try_from(params.ttl).map_err(|_| einval())?;
    if version == 6 {
        sock.set_tclass_v6(tos)?;
        sock.set_unicast_hops_v6(ttl)?;
    } else {
        sock.set_tos_v4(tos)?;
        sock.set_ttl_v4(ttl)?;
    }
    Ok(())
}

/// Per-probe options on a shared send socket: the common block plus the bind C does between
/// the device and the TOS calls. `local: None` means "do not bind" — the stream path binds
/// itself to `local:sequence` after setting `SO_REUSEPORT`/`SO_REUSEADDR` (Task 13). Setting
/// TOS/TTL after the bind rather than around it is observably identical: they are socket-level
/// options unaffected by binding.
pub fn apply_probe_options(
    sock: &Socket,
    version: u8,
    params: &CProbeParams,
    local: Option<SocketAddr>,
    is_raw: bool,
) -> std::io::Result<()> {
    if let Some(local) = local {
        let already_bound = match sock.local_addr()?.as_socket() {
            Some(cur) if is_raw => cur == local,
            Some(cur) => cur.port() != 0,
            None => false,
        };
        if !already_bound {
            sock.bind(&SockAddr::from(local))?;
        }
    }
    set_common_options(sock, version, params)
}

/// Loopback tests gate on this: with neither `cap_net_raw` nor open ping sockets there is no
/// probe socket at all, and the test returns early instead of failing (Global Constraints).
#[cfg(test)]
pub(crate) fn dgram_available(version: u8) -> bool {
    Family::open(version).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_opens_raw_or_falls_back_to_dgram_with_recverr() {
        if !dgram_available(4) {
            eprintln!("skipping: no cap_net_raw and no open ping sockets");
            return;
        }
        let f = Family::open(4).expect("ping sockets are open to all groups on this box");
        assert_eq!(f.version, 4);
        // Unprivileged test processes get the DGRAM pair; with cap_net_raw the raw triple.
        match &f.sockets {
            Sockets::Dgram { icmp, udp } => {
                assert_eq!(icmp.protocol().unwrap(), Some(socket2::Protocol::ICMPV4));
                assert_eq!(udp.protocol().unwrap(), Some(socket2::Protocol::UDP));
                assert_eq!(f.recv_fds().len(), 2);
            }
            Sockets::Raw { .. } => assert_eq!(f.recv_fds().len(), 1),
        }
        f.set_nonblocking().unwrap();
    }

    #[test]
    fn ipv6_opens_too_on_this_box() {
        // The box has link-local + ULA IPv6 but no global address; opening the sockets and
        // reaching `::1` works regardless, which is all this test and the v6 loopback tests need.
        if !dgram_available(6) {
            eprintln!("skipping: no IPv6 probe sockets");
            return;
        }
        let f = Family::open(6).expect("IPv6 sockets open (loopback IPv6 is always present)");
        assert_eq!(f.version, 6);
    }

    #[test]
    fn sctp_support_is_detected() {
        assert!(
            check_sctp_support(),
            "the sctp module is loaded on this box"
        );
    }

    #[test]
    fn probe_options_apply_to_a_dgram_socket() {
        if !dgram_available(4) {
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
