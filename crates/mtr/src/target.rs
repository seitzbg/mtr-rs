//! Target and local endpoint: get_addrinfo_from_name (ui/mtr.c:1043-1072),
//! net_find_local_address / -a / -I (ui/net.c:656-785), LocalHostname (ui/mtr.c:1231-1234) —
//! mtr 0.96, commit 7b01773. GPL-2.0-only.

use std::net::{IpAddr, SocketAddr};

use crate::cli::AddressFamily;

fn family_ok(ip: IpAddr, af: AddressFamily) -> bool {
    match af {
        AddressFamily::Unspec => true,
        AddressFamily::V4 => ip.is_ipv4(),
        AddressFamily::V6 => ip.is_ipv6(),
    }
}

/// `unmap_v4mapped_addrinfo()` (ui/mtr.c:1007): `::ffff:a.b.c.d` becomes `a.b.c.d`, but only
/// when no address family was requested — with `-4`/`-6` the address is used as resolved.
fn unmap(ip: IpAddr, af: AddressFamily) -> IpAddr {
    match ip {
        IpAddr::V6(v6) if af == AddressFamily::Unspec => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        other => other,
    }
}

/// First `getaddrinfo()` result in the requested family.
pub async fn resolve_target(name: &str, af: AddressFamily) -> Result<IpAddr, String> {
    tracing::debug!(host = name, "resolve");
    if let Ok(ip) = name.parse::<IpAddr>() {
        let ip = unmap(ip, af);
        return if family_ok(ip, af) {
            Ok(ip)
        } else {
            Err(format!("Failed to resolve host: {name}"))
        };
    }
    let addrs = tokio::net::lookup_host((name, 0u16))
        .await
        .map_err(|e| format!("Failed to resolve host: {name}: {e}"))?;
    addrs
        .map(|s| unmap(s.ip(), af))
        .find(|ip| family_ok(*ip, af))
        .ok_or_else(|| format!("Failed to resolve host: {name}"))
}

/// `net_find_local_address()` (net.c:732-785): connect a UDP socket to the target on port 1 and
/// read the source address the kernel chose. `EHOSTUNREACH` is not fatal (Linux special case).
pub fn find_local_address(target: IpAddr, mark: u32) -> Result<Option<IpAddr>, String> {
    use socket2::{Domain, Protocol, SockAddr, Socket, Type};
    let domain = if target.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let sock = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))
        .map_err(|e| format!("udp socket creation failed: {e}"))?;
    #[cfg(target_os = "linux")]
    if mark != 0 {
        sock.set_mark(mark).map_err(|e| {
            format!("setsockopt SO_MARK failed: {e} (--mark needs CAP_NET_ADMIN: run mtr as root)")
        })?;
    }
    #[cfg(not(target_os = "linux"))]
    let _ = mark;
    match sock.connect(&SockAddr::from(SocketAddr::new(target, 1))) {
        Ok(()) => {}
        // net.c:764-773: only Linux tolerates an unreachable target here.
        Err(e) if cfg!(target_os = "linux") && e.kind() == std::io::ErrorKind::HostUnreachable => {
            return Ok(None);
        }
        Err(e) => return Err(format!("udp socket connect failed: {e}")),
    }
    let local = sock
        .local_addr()
        .map_err(|e| format!("getsockname failed: {e}"))?;
    Ok(local.as_socket().map(|s| s.ip()))
}

/// `-a` (`net_validate_interface_address`): must be in the target's family.
pub fn validate_source_address(addr: IpAddr, target: IpAddr) -> Result<IpAddr, String> {
    if addr.is_ipv4() == target.is_ipv4() {
        Ok(addr)
    } else {
        Err("invalid local address".to_string())
    }
}

/// `-I` (`net_find_interface_address_from_name`, net.c:677-724).
pub fn interface_address(name: &str, want_v6: bool) -> Result<IpAddr, String> {
    let addrs = nix::ifaddrs::getifaddrs().map_err(|e| format!("getifaddrs failed: {e}"))?;
    let mut found_interface = false;
    for ifa in addrs {
        if ifa.interface_name != name {
            continue;
        }
        let Some(ss) = ifa.address else {
            // net.c:692: the name match only counts when the entry carries an address.
            continue;
        };
        found_interface = true;
        match (want_v6, ss.as_sockaddr_in(), ss.as_sockaddr_in6()) {
            (false, Some(sin), _) => return Ok(IpAddr::V4(sin.ip())),
            (true, _, Some(sin6)) => return Ok(IpAddr::V6(sin6.ip())),
            _ => {}
        }
    }
    if found_interface {
        Err(format!(
            "interface missing {} address",
            if want_v6 { "IPv6" } else { "IPv4" }
        ))
    } else {
        Err("no such interface".to_string())
    }
}

/// `gethostname()` with C's fallback.
pub fn local_hostname() -> String {
    nix::unistd::gethostname()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "UNKNOWNHOST".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn literals_resolve_without_dns_and_respect_the_family() {
        assert_eq!(
            resolve_target("127.0.0.1", AddressFamily::Unspec)
                .await
                .unwrap(),
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            resolve_target("::1", AddressFamily::V6).await.unwrap(),
            "::1".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            resolve_target("::1", AddressFamily::V4).await.unwrap_err(),
            "Failed to resolve host: ::1"
        );
        assert_eq!(
            resolve_target("::ffff:192.0.2.1", AddressFamily::Unspec)
                .await
                .unwrap(),
            "192.0.2.1".parse::<IpAddr>().unwrap()
        );
        // ui/mtr.c:1007: an explicit family keeps the v4-mapped address as an IPv6 target
        assert_eq!(
            resolve_target("::ffff:192.0.2.1", AddressFamily::V6)
                .await
                .unwrap(),
            "::ffff:192.0.2.1".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            resolve_target("::ffff:192.0.2.1", AddressFamily::V4)
                .await
                .unwrap_err(),
            "Failed to resolve host: ::ffff:192.0.2.1"
        );
    }

    #[tokio::test]
    async fn localhost_resolves_to_a_loopback_address() {
        let ip = resolve_target("localhost", AddressFamily::Unspec)
            .await
            .unwrap();
        assert!(ip.is_loopback(), "{ip}");
        let ip = resolve_target("localhost", AddressFamily::V4)
            .await
            .unwrap();
        assert_eq!(ip, "127.0.0.1".parse::<IpAddr>().unwrap());
    }

    #[tokio::test]
    async fn unresolvable_names_fail_with_c_message() {
        let err = resolve_target("no-such-host.invalid", AddressFamily::Unspec)
            .await
            .unwrap_err();
        assert!(
            err.starts_with("Failed to resolve host: no-such-host.invalid"),
            "{err}"
        );
    }

    #[test]
    fn local_address_for_loopback_target_is_loopback() {
        let local = find_local_address("127.0.0.1".parse().unwrap(), 0).unwrap();
        assert_eq!(local, Some("127.0.0.1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn source_address_must_match_the_target_family() {
        let v4: IpAddr = "192.0.2.1".parse().unwrap();
        let v6: IpAddr = "2001:db8::1".parse().unwrap();
        assert_eq!(validate_source_address(v4, v4), Ok(v4));
        assert_eq!(
            validate_source_address(v6, v4),
            Err("invalid local address".to_string())
        );
    }

    #[test]
    fn interface_lookup_reports_missing_interfaces() {
        assert_eq!(
            interface_address("no-such-if0", false),
            Err("no such interface".to_string())
        );
        // Linux names loopback `lo`, the BSDs `lo0`.
        let name = if cfg!(target_os = "linux") {
            "lo"
        } else {
            "lo0"
        };
        let lo = interface_address(name, false).unwrap();
        assert!(lo.is_loopback());
    }

    #[test]
    fn hostname_is_non_empty() {
        assert!(!local_hostname().is_empty());
    }
}
