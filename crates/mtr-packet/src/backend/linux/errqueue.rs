//! `MSG_ERRQUEUE` reads for the unprivileged DGRAM sockets. Ported from
//! packet/probe_unix.c:704-844 (mtr 0.96, commit 7b01773). GPL-2.0-only.

use std::io::IoSliceMut;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::fd::AsRawFd;

use nix::sys::socket::{ControlMessageOwned, MsgFlags, SockaddrStorage, recvmsg};
use socket2::Socket;

/// Which of C's two error-queue cases (probe_unix.c:772-789) an entry belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuedError {
    /// An ICMP time-exceeded for our datagram (v4 type 11, v6 type 3) — errno `EHOSTUNREACH`.
    TimeExceeded,
    /// Port unreachable (v4 type 3 code 3, v6 type 1 code 4): the datagram reached its
    /// destination — errno `ECONNREFUSED`.
    Refused,
    /// Any other destination-unreachable: `no-route-host`.
    Unreachable,
}

/// After `recv_from` failed with `EHOSTUNREACH`/`ECONNREFUSED`, read the queued error: the
/// offender (the router or host that sent the ICMP) from the `IP_RECVERR`/`IPV6_RECVERR`
/// control message (probe_unix.c:794-812) and the original payload into `buf`. `fallback` is
/// the errno-derived guess; Task 11 replaces it with the exact ICMP type/code carried in the
/// same control message. There is no `version` parameter: the cmsg variant
/// (`Ipv4RecvErr`/`Ipv6RecvErr`) already tells us the family.
///
/// `Ok(None)` means the queue was empty (`EAGAIN`) or carried no offender address, which is
/// C's `if (ee)` guard at probe_unix.c:810: without an offender there is nobody to report.
/// The socket is non-blocking, so this never waits.
pub fn read_error(
    sock: &Socket,
    buf: &mut [u8],
    fallback: QueuedError,
) -> std::io::Result<Option<(IpAddr, usize, QueuedError)>> {
    let mut cmsg = nix::cmsg_space!(nix::libc::sock_extended_err, nix::libc::sockaddr_in6);
    let mut iov = [IoSliceMut::new(buf)];
    let msg = match recvmsg::<SockaddrStorage>(
        sock.as_raw_fd(),
        &mut iov,
        Some(&mut cmsg),
        MsgFlags::MSG_ERRQUEUE,
    ) {
        Ok(m) => m,
        Err(nix::errno::Errno::EAGAIN) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut offender = None;
    for c in msg.cmsgs()? {
        match c {
            ControlMessageOwned::Ipv4RecvErr(_, Some(sin)) => {
                offender = Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(
                    sin.sin_addr.s_addr,
                ))));
            }
            ControlMessageOwned::Ipv6RecvErr(_, Some(sin6)) => {
                offender = Some(IpAddr::V6(Ipv6Addr::from(sin6.sin6_addr.s6_addr)));
            }
            _ => {}
        }
    }
    Ok(offender.map(|a| (a, msg.bytes, fallback)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use socket2::{Domain, Protocol, Type};

    /// An empty error queue on a non-blocking socket is `Ok(None)`, not an error and not a
    /// block: `drain_socket` relies on that to end a socket's turn.
    #[test]
    fn an_empty_error_queue_reads_as_none_without_blocking() {
        let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
        sock.set_nonblocking(true).unwrap();
        let mut buf = [0u8; 128];
        assert_eq!(
            read_error(&sock, &mut buf, QueuedError::TimeExceeded).unwrap(),
            None
        );
    }

    /// A real queued error, produced without any privilege: connect a UDP socket to a closed
    /// loopback port, send, and let the kernel's own ICMP port-unreachable land on the queue.
    #[test]
    fn a_port_unreachable_comes_back_with_its_offender_and_payload() {
        let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
        nix::sys::socket::setsockopt(&sock, nix::sys::socket::sockopt::Ipv4RecvErr, &true).unwrap();
        // Port 1 on loopback has no listener, so the kernel answers its own datagram.
        let dst: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
        sock.connect(&socket2::SockAddr::from(dst)).unwrap();
        sock.send(b"probe payload").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        sock.set_nonblocking(true).unwrap();
        let mut buf = [0u8; 128];
        // The queued error surfaces on the next ordinary read as ECONNREFUSED.
        let mut junk = [0u8; 128];
        let e = nix::sys::socket::recvfrom::<SockaddrStorage>(sock.as_raw_fd(), &mut junk)
            .expect_err("the queued error is reported first");
        assert_eq!(e, nix::errno::Errno::ECONNREFUSED);
        let (offender, n, kind) = read_error(&sock, &mut buf, QueuedError::Refused)
            .unwrap()
            .expect("the offender is loopback itself");
        assert_eq!(offender, "127.0.0.1".parse::<IpAddr>().unwrap());
        assert_eq!(&buf[..n], b"probe payload");
        assert_eq!(kind, QueuedError::Refused);
        // The queue is drained now.
        assert_eq!(
            read_error(&sock, &mut buf, QueuedError::Refused).unwrap(),
            None
        );
    }
}
