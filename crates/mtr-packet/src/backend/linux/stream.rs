//! TCP and SCTP connect probes. Ported from packet/construct_unix.c:319-480 and
//! packet/probe_unix.c:846-904 (mtr 0.96, commit 7b01773). GPL-2.0-only.
//!
//! A stream probe is its own socket: `connect()` from `local:sequence` to `remote:port` with
//! `O_NONBLOCK`, so the SYN goes out and the call returns `EINPROGRESS`. The source port is the
//! probe's sequence number, which is how an ICMP time-exceeded quoting our SYN is matched back
//! (construct_unix.c:444-451, `Inner::Stream` in deconstruct.rs). The socket lives in
//! `Probe::stream` until the connect completes, fails, or the probe times out, and is closed by
//! dropping the probe.

use std::net::{IpAddr, SocketAddr};
use std::os::fd::AsFd;

use mtr_proto::{CProbeParams, Protocol};
use nix::poll::{PollFd, PollFlags, PollTimeout};
use socket2::{Domain, SockAddr, Socket, Type};

/// `HTTP_PORT` (construct_unix.c:63): the destination when the request named none.
pub const HTTP_PORT: u16 = 80;

/// What `receive_replies_from_probe_socket()` (probe_unix.c:851-904) decides about a probe.
#[derive(Debug)]
pub enum Completion {
    /// Not writable yet: the connect is still in flight.
    Pending,
    /// Connected, or refused — either way the destination answered.
    Reached,
    /// `SO_ERROR` says the probe failed; the caller reports it and frees the probe.
    Failed(std::io::Error),
}

fn einval() -> std::io::Error {
    std::io::Error::from_raw_os_error(nix::libc::EINVAL)
}

/// The destination port of a stream probe (construct_unix.c:465-471): the requested one, or
/// http when the request named none. `dest_port` is an unvalidated C `int`, and C narrows it in
/// `htons()`; the UDP path truncates the same way (construct.rs `udp_ports`), so this does too.
pub fn dest_port(params: &CProbeParams) -> u16 {
    if params.dest_port != 0 {
        params.dest_port as u16
    } else {
        HTTP_PORT
    }
}

/// `set_stream_socket_options()` (construct_unix.c:324-406): the two reuse flags, which only
/// the stream path sets, and then the shared mark/device/TOS/TTL block from `sockets.rs`. C
/// applies TTL and TOS before the mark and the device; the four are independent `setsockopt`
/// calls on a socket that has not been bound yet, so the order is immaterial and there stays
/// exactly one copy of them (pre-flight ruling 12).
fn set_options(sock: &Socket, version: u8, params: &CProbeParams) -> std::io::Result<()> {
    sock.set_reuse_port(true)?;
    sock.set_reuse_address(true)?;
    super::sockets::set_common_options(sock, version, params)
}

/// `open_stream_socket()` (construct_unix.c:410-480): bind `local:sequence` so a returned ICMP
/// can be matched, then start the connect. `EINPROGRESS` is the normal outcome of a
/// non-blocking connect and counts as success; every other errno is the caller's to handle
/// (`EADDRINUSE`/`EADDRNOTAVAIL` retry, `ECONNREFUSED` is an immediate reply).
pub fn open(
    protocol: Protocol,
    version: u8,
    sequence: u16,
    local: IpAddr,
    remote: IpAddr,
    params: &CProbeParams,
) -> std::io::Result<(Socket, SocketAddr)> {
    // construct_unix.c:428-436: only 4 and 6 are addressable families.
    let domain = match version {
        4 => Domain::IPV4,
        6 => Domain::IPV6,
        _ => return Err(einval()),
    };
    let proto = match protocol {
        Protocol::Tcp => socket2::Protocol::TCP,
        Protocol::Sctp => socket2::Protocol::SCTP,
        _ => return Err(einval()),
    };
    // The local and remote addresses must agree with the family we are opening; a mismatch
    // would otherwise reach bind() as EAFNOSUPPORT/EINVAL from the kernel.
    if (version == 6) != local.is_ipv6() || (version == 6) != remote.is_ipv6() {
        return Err(einval());
    }
    let sock = Socket::new(domain, Type::STREAM, Some(proto))?;
    sock.set_nonblocking(true)?;
    set_options(&sock, version, params)?;
    sock.bind(&SockAddr::from(SocketAddr::new(local, sequence)))?;
    let dest = SocketAddr::new(remote, dest_port(params));
    match sock.connect(&SockAddr::from(dest)) {
        Ok(()) => Ok((sock, dest)),
        Err(e) if e.raw_os_error() == Some(nix::libc::EINPROGRESS) => Ok((sock, dest)),
        Err(e) => Err(e),
    }
}

/// `receive_replies_from_probe_socket()` (probe_unix.c:851-904): a writable socket means the
/// connect attempt has finished, and `SO_ERROR` says how. C's `select()` with a zero timeout is
/// a `poll()` with `PollTimeout::ZERO` here. `EAGAIN` from the poll is "nothing yet"
/// (probe_unix.c:876-878); so is `EINTR`, which C would treat as fatal but which only means the
/// answer has not been read yet.
///
/// C's `select()` marks a connecting socket writable both when the connect succeeds and when it
/// fails (Linux's `sock_def_write_space()` wakes writers on either outcome, which is what
/// `write_set` observes). `poll(2)` is not required to make the same promise: on this kernel a
/// refused *SCTP* connect (unlike TCP's) reports only `POLLERR`, never `POLLOUT`, on the failed
/// socket. Treating `POLLERR` as completion too keeps `poll()` matching `select()`'s semantics
/// for both protocols, rather than spinning forever on the SCTP case.
pub fn check(sock: &Socket) -> Completion {
    let mut fds = [PollFd::new(sock.as_fd(), PollFlags::POLLOUT)];
    match nix::poll::poll(&mut fds, PollTimeout::ZERO) {
        Ok(0) => return Completion::Pending,
        Ok(_) => {}
        Err(_) => return Completion::Pending,
    }
    if !fds[0]
        .revents()
        .is_some_and(|r| r.intersects(PollFlags::POLLOUT | PollFlags::POLLERR))
    {
        return Completion::Pending;
    }
    // probe_unix.c:896-903: connected, or refused, both mean the probe arrived.
    match sock.take_error() {
        Ok(None) => Completion::Reached,
        Ok(Some(e)) if e.raw_os_error() == Some(nix::libc::ECONNREFUSED) => Completion::Reached,
        Ok(Some(e)) => Completion::Failed(e),
        Err(e) => Completion::Failed(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(port: u16) -> CProbeParams {
        CProbeParams {
            protocol: Protocol::Tcp,
            dest_port: i32::from(port),
            ..Default::default()
        }
    }

    /// `open()` from a fixed source port, walking forward past ports this box already uses —
    /// exactly what `send_probe()`'s retry loop does (probe_unix.c:588-608).
    fn open_from_a_free_port(
        first: u16,
        remote: IpAddr,
        p: &CProbeParams,
    ) -> std::io::Result<(Socket, SocketAddr)> {
        let mut last = Err(einval());
        for seq in first..first + 50 {
            last = open(
                Protocol::Tcp,
                4,
                seq,
                "127.0.0.1".parse().unwrap(),
                remote,
                p,
            );
            match &last {
                Err(e)
                    if matches!(
                        e.raw_os_error(),
                        Some(nix::libc::EADDRINUSE | nix::libc::EADDRNOTAVAIL)
                    ) => {}
                _ => break,
            }
        }
        last
    }

    /// Poll `check()` until it stops saying `Pending`; a loopback connect settles in
    /// microseconds, so a second of patience is plenty.
    fn settle(sock: &Socket) -> Completion {
        for _ in 0..100 {
            match check(sock) {
                Completion::Pending => std::thread::sleep(std::time::Duration::from_millis(10)),
                other => return other,
            }
        }
        panic!("the connect never completed");
    }

    #[test]
    fn connect_to_a_listener_completes_and_to_a_closed_port_is_refused() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = params(l.local_addr().unwrap().port());
        let (s, remote) = open_from_a_free_port(40000, "127.0.0.1".parse().unwrap(), &p).unwrap();
        assert_eq!(remote.port(), l.local_addr().unwrap().port());
        assert!(matches!(settle(&s), Completion::Reached), "listening port");

        // Port 1 has no listener: the connect is refused, which still means "reached".
        let closed = params(1);
        match open_from_a_free_port(40100, "127.0.0.1".parse().unwrap(), &closed) {
            // Linux answers its own RST asynchronously; FreeBSD refuses right in connect()
            // (probe_unix.c:610-616), and both are acceptable here.
            Err(e) => assert_eq!(e.raw_os_error(), Some(nix::libc::ECONNREFUSED), "{e}"),
            Ok((s, _)) => assert!(matches!(settle(&s), Completion::Reached), "closed port"),
        }
    }

    /// The source port is the sequence number, and it is the one the peer sees — that is the
    /// whole reason for binding (construct_unix.c:444-451).
    #[test]
    fn the_probe_binds_the_sequence_as_its_source_port() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = params(l.local_addr().unwrap().port());
        let (s, _) = open_from_a_free_port(40200, "127.0.0.1".parse().unwrap(), &p).unwrap();
        let bound = s.local_addr().unwrap().as_socket().unwrap().port();
        assert!((40200..40250).contains(&bound), "bound to {bound}");
        let (peer, _) = l.accept().unwrap();
        assert_eq!(peer.peer_addr().unwrap().port(), bound);
    }

    /// A port already held by a socket without `SO_REUSEPORT` cannot be shared, so `open()`
    /// hands back `EADDRINUSE` for `send_probe()` to retry on.
    #[test]
    fn a_busy_source_port_is_reported_as_address_in_use() {
        let blocker = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let busy = blocker.local_addr().unwrap().port();
        let e = open(
            Protocol::Tcp,
            4,
            busy,
            "127.0.0.1".parse().unwrap(),
            "127.0.0.1".parse().unwrap(),
            &params(80),
        )
        .expect_err("the port is taken");
        assert_eq!(e.raw_os_error(), Some(nix::libc::EADDRINUSE), "{e}");
        drop(blocker);
    }

    #[test]
    fn only_stream_protocols_and_real_families_open() {
        let local: IpAddr = "127.0.0.1".parse().unwrap();
        for (proto, version) in [
            (Protocol::Icmp, 4),
            (Protocol::Udp, 4),
            (Protocol::Tcp, 0),
            (Protocol::Tcp, 6), // an IPv4 address in an IPv6 socket
        ] {
            let e = open(proto, version, 40300, local, local, &params(80))
                .expect_err("{proto:?}/{version}");
            assert_eq!(e.raw_os_error(), Some(nix::libc::EINVAL), "{proto:?}");
        }
    }

    #[test]
    fn the_default_destination_port_is_http() {
        assert_eq!(dest_port(&params(0)), HTTP_PORT);
        assert_eq!(dest_port(&params(443)), 443);
        // Out of range truncates in `htons()`, exactly as C and the UDP path do.
        assert_eq!(
            dest_port(&CProbeParams {
                dest_port: 70000,
                ..Default::default()
            }),
            70000u32 as u16
        );
    }
}
