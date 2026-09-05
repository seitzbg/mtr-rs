//! Reply parsing and probe matching. Ported from packet/deconstruct_unix.c (mtr 0.96, commit
//! 7b01773). GPL-2.0-only.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use mtr_proto::{MplsLabel, ProbeResult, Protocol};

use super::construct::{ICMP_HEADER, IP4_HEADER, IP6_HEADER, UDP_HEADER};
use crate::probe_table::ProbeTable;

/// `MAX_MPLS_LABELS` (deconstruct_unix.c:28).
pub const MAX_MPLS_LABELS: usize = 8;
/// The reply encoder sizes its label array from `mtr_proto::MAX_LABELS`; the two must agree or
/// a decoded label could be dropped (or the encoder overrun its own bound) silently.
const _: () = assert!(MAX_MPLS_LABELS == mtr_proto::MAX_LABELS);
/// `ICMP_ORIGINAL_DATAGRAM_MIN_SIZE` (protocols.h:44).
const ICMP_ORIGINAL_DATAGRAM_MIN_SIZE: usize = 128;
/// `sizeof(struct ICMPExtensionHeader)` / `sizeof(struct ICMPExtensionObject)`, both 4 bytes
/// (protocols.h:65-76).
const ICMP_EXT_HEADER: usize = 4;
const ICMP_EXT_OBJECT: usize = 4;
/// `sizeof(struct TCPHeader)` == `sizeof(struct SCTPHeader)` (protocols.h:93-105).
const STREAM_HEADER: usize = 8;
/// `ICMP_EXT_MPLS_CLASSNUM` / `ICMP_EXT_MPLS_CTYPE` (protocols.h:47-48).
const ICMP_EXT_MPLS_CLASSNUM: u8 = 1;
const ICMP_EXT_MPLS_CTYPE: u8 = 1;
// protocols.h:22-38 — `pub` because the error-queue path (Task 11) decodes the same numbers out
// of the `sock_extended_err` control message; there is one definition of each.
pub const ICMP_ECHOREPLY: u8 = 0;
pub const ICMP_DEST_UNREACH: u8 = 3;
pub const ICMP_TIME_EXCEEDED: u8 = 11;
pub const ICMP_PORT_UNREACH: u8 = 3;
pub const ICMP6_DEST_UNREACH: u8 = 1;
pub const ICMP6_TIME_EXCEEDED: u8 = 3;
pub const ICMP6_ECHOREPLY: u8 = 129;
pub const ICMP6_PORT_UNREACH: u8 = 4;
const IPPROTO_ICMP: u8 = 1;
const IPPROTO_TCP: u8 = 6;
const IPPROTO_UDP: u8 = 17;
const IPPROTO_ICMPV6: u8 = 58;
const IPPROTO_SCTP: u8 = 132;

/// The three ICMP messages `handle_received_icmp4_packet()` (deconstruct_unix.c:404-467) and
/// `handle_received_icmp6_packet()` (:474-539) act on; everything else is dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IcmpKind {
    EchoReply,
    TimeExceeded,
    DestUnreach { code: u8 },
}

/// The headers of the original datagram quoted back inside a time-exceeded or destination
/// unreachable message (`handle_inner_ip4_packet()`, deconstruct_unix.c:152-221).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inner {
    Icmp {
        id: u16,
        sequence: u16,
    },
    Udp {
        src: IpAddr,
        dst: IpAddr,
        src_port: u16,
        dst_port: u16,
        checksum: u16,
    },
    /// TCP or SCTP: the sequence number is the source port (deconstruct_unix.c:203-219).
    Stream {
        src_port: u16,
    },
}

/// Everything `match_reply()` needs out of a received ICMP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub kind: IcmpKind,
    /// `(id, sequence)` of an echo reply; `None` for the quoting messages.
    pub echo: Option<(u16, u16)>,
    pub inner: Option<Inner>,
    pub mpls: Vec<MplsLabel>,
}

/// Network-order 16-bit read; `None` when `at + 2 > b.len()`, so a caller can never index past
/// the end even if its own length check is wrong.
fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([
        *b.get(at)?,
        *b.get(at.checked_add(1)?)?,
    ]))
}

/// `decode_mpls_labels()` (deconstruct_unix.c:337-394) plus `decode_mpls_object()` (:299-331).
/// `icmp` starts at the ICMP header; the RFC 4884 extension follows the 128-byte minimum
/// "original datagram" area.
pub fn decode_mpls(icmp: &[u8]) -> Vec<MplsLabel> {
    let ext_at = ICMP_HEADER + ICMP_ORIGINAL_DATAGRAM_MIN_SIZE;
    // deconstruct_unix.c:355-357, :361-363: enough room for the extension header, version 2.
    if icmp.len() < ext_at + ICMP_EXT_HEADER || icmp[ext_at] & 0xF0 != 0x20 {
        return Vec::new();
    }
    // deconstruct_unix.c:371-393: walk the object chain looking for the MPLS object.
    let mut objs = &icmp[ext_at + ICMP_EXT_HEADER..];
    while objs.len() >= ICMP_EXT_OBJECT {
        let Some(len) = be16(objs, 0) else { break };
        let len = usize::from(len);
        if len > objs.len() || len < ICMP_EXT_OBJECT {
            return Vec::new();
        }
        if objs[2] == ICMP_EXT_MPLS_CLASSNUM && objs[3] == ICMP_EXT_MPLS_CTYPE {
            return objs[ICMP_EXT_OBJECT..len]
                .as_chunks::<4>()
                .0
                .iter()
                .take(MAX_MPLS_LABELS)
                .map(|l| MplsLabel {
                    label: (u32::from(l[0]) << 12)
                        | (u32::from(l[1]) << 4)
                        | (u32::from(l[2]) >> 4),
                    tc: (l[2] & 0x0E) >> 1,
                    bottom_of_stack: l[2] & 0x01 != 0,
                    ttl: l[3],
                })
                .collect();
        }
        objs = &objs[len..];
    }
    Vec::new()
}

/// Deviation 32: the length of an IPv4 header, taken from its IHL field. C
/// (`deconstruct_unix.c:167`, :556) hardcodes `sizeof(struct IPHeader)` == 20, so a packet — or a
/// quoted original datagram — carrying IP options is parsed at the wrong offset. `None` when the
/// buffer is too short for a minimal header, the version nibble is not 4, IHL < 5, or the header
/// would run past the end of the buffer.
fn ip4_header_len(ip: &[u8]) -> Option<usize> {
    if ip.len() < IP4_HEADER || ip[0] >> 4 != 4 {
        return None;
    }
    let len = usize::from(ip[0] & 0x0F) * 4;
    if len < IP4_HEADER || len > ip.len() {
        return None;
    }
    Some(len)
}

/// `handle_inner_ip4_packet()` (deconstruct_unix.c:152-221): the original datagram's headers.
fn inner4(ip: &[u8]) -> Option<Inner> {
    let header_len = ip4_header_len(ip)?;
    if ip.len() < header_len + ICMP_HEADER {
        return None;
    }
    let src = IpAddr::V4(Ipv4Addr::new(ip[12], ip[13], ip[14], ip[15]));
    let dst = IpAddr::V4(Ipv4Addr::new(ip[16], ip[17], ip[18], ip[19]));
    inner_transport(ip[9], src, dst, &ip[header_len..])
}

/// `handle_inner_ip6_packet()` (deconstruct_unix.c:227-295).
fn inner6(ip: &[u8]) -> Option<Inner> {
    if ip.len() < IP6_HEADER + ICMP_HEADER {
        return None;
    }
    let mut s = [0u8; 16];
    let mut d = [0u8; 16];
    s.copy_from_slice(&ip[8..24]);
    d.copy_from_slice(&ip[24..40]);
    inner_transport(
        ip[6],
        IpAddr::V6(Ipv6Addr::from(s)),
        IpAddr::V6(Ipv6Addr::from(d)),
        &ip[IP6_HEADER..],
    )
}

/// The protocol dispatch shared by both `handle_inner_ip*_packet()`; `t` starts at the
/// transport header. Each arm re-checks the length the C code checks against its `ip_*_size`.
fn inner_transport(proto: u8, src: IpAddr, dst: IpAddr, t: &[u8]) -> Option<Inner> {
    match proto {
        IPPROTO_ICMP | IPPROTO_ICMPV6 if t.len() >= ICMP_HEADER => Some(Inner::Icmp {
            id: be16(t, 4)?,
            sequence: be16(t, 6)?,
        }),
        IPPROTO_UDP if t.len() >= UDP_HEADER => Some(Inner::Udp {
            src,
            dst,
            src_port: be16(t, 0)?,
            dst_port: be16(t, 2)?,
            checksum: be16(t, 6)?,
        }),
        IPPROTO_TCP | IPPROTO_SCTP if t.len() >= STREAM_HEADER => Some(Inner::Stream {
            src_port: be16(t, 0)?,
        }),
        _ => None,
    }
}

/// `handle_received_icmp4_packet()` (deconstruct_unix.c:404-467) and
/// `handle_received_icmp6_packet()` (:474-539), which differ only in their type constants and
/// in the size of the quoted IP header.
fn parse_icmp(icmp: &[u8], version: u8) -> Option<Parsed> {
    if icmp.len() < ICMP_HEADER {
        return None;
    }
    let (echo_t, exceeded_t, unreach_t) = if version == 6 {
        (ICMP6_ECHOREPLY, ICMP6_TIME_EXCEEDED, ICMP6_DEST_UNREACH)
    } else {
        (ICMP_ECHOREPLY, ICMP_TIME_EXCEEDED, ICMP_DEST_UNREACH)
    };
    let mpls = decode_mpls(icmp);
    let body = &icmp[ICMP_HEADER..];
    let kind = match icmp[0] {
        t if t == echo_t => {
            return Some(Parsed {
                kind: IcmpKind::EchoReply,
                echo: Some((be16(icmp, 4)?, be16(icmp, 6)?)),
                inner: None,
                mpls,
            });
        }
        t if t == exceeded_t => IcmpKind::TimeExceeded,
        t if t == unreach_t => IcmpKind::DestUnreach { code: icmp[1] },
        _ => return None,
    };
    let inner = if version == 6 {
        inner6(body)
    } else {
        inner4(body)
    };
    Some(Parsed {
        kind,
        echo: None,
        inner,
        mpls,
    })
}

/// `handle_received_ip4_packet()` (deconstruct_unix.c:546-583): raw sockets include the IPv4
/// header, the unprivileged DGRAM socket does not.
pub fn parse_icmp4(packet: &[u8], has_ip_header: bool) -> Option<Parsed> {
    if has_ip_header {
        let header_len = ip4_header_len(packet)?;
        if packet.len() < header_len + ICMP_HEADER || packet[9] != IPPROTO_ICMP {
            return None;
        }
        parse_icmp(&packet[header_len..], 4)
    } else {
        parse_icmp(packet, 4)
    }
}

/// `handle_received_ip6_packet()` (deconstruct_unix.c:590-604): ICMPv6 sockets hand us the ICMP
/// header first, with no IPv6 header of their own.
pub fn parse_icmp6(packet: &[u8]) -> Option<Parsed> {
    parse_icmp(packet, 6)
}

/// `ICMP_PORT_UNREACH` (3) / `ICMP6_PORT_UNREACH` (4), protocols.h:22-38. Public because the
/// error-queue path (Tasks 10-12) synthesises a `DestUnreach` with exactly this code when the
/// kernel reports `ECONNREFUSED`.
pub fn port_unreach_code(version: u8) -> u8 {
    if version == 6 {
        ICMP6_PORT_UNREACH
    } else {
        ICMP_PORT_UNREACH
    }
}

/// `find_and_receive_probe()` (deconstruct_unix.c:34-54) over `find_probe()` (probe.c:176-205),
/// with the result word `handle_received_icmp*_packet()` picks: an echo reply or a port
/// unreachable is `reply`, a time exceeded is `ttl-expired`, and any other destination
/// unreachable code is `no-route-host` (deconstruct_unix.c:420-466).
pub fn match_reply(
    table: &ProbeTable,
    parsed: &Parsed,
    version: u8,
    icmp_id: u16,
) -> Option<(usize, ProbeResult)> {
    let result = match &parsed.kind {
        IcmpKind::EchoReply => ProbeResult::Reply,
        IcmpKind::TimeExceeded => ProbeResult::TtlExpired,
        IcmpKind::DestUnreach { code } if *code == port_unreach_code(version) => ProbeResult::Reply,
        IcmpKind::DestUnreach { .. } => ProbeResult::NoRouteHost,
    };
    // probe.c:186-193: an ICMP id that isn't ours means the reply belongs to another process.
    if let Some((id, seq)) = parsed.echo {
        if id != icmp_id {
            return None;
        }
        return table.find_by_sequence(seq).map(|i| (i, result));
    }
    match parsed.inner.as_ref()? {
        Inner::Icmp { id, sequence } => {
            if *id != icmp_id {
                return None;
            }
            table.find_by_sequence(*sequence).map(|i| (i, result))
        }
        Inner::Udp {
            src,
            dst,
            src_port,
            dst_port,
            checksum,
        } => match_udp(table, *src_port, *dst_port, *checksum, Some((*src, *dst)))
            .map(|i| (i, result)),
        Inner::Stream { src_port } => table.find_by_sequence(*src_port).map(|i| (i, result)),
    }
}

/// `handle_inner_udp_packet()` (deconstruct_unix.c:63-121). The single home of the UDP matching
/// rules: try the destination port, the source port and the checksum field as the sequence, then
/// require the ports (and, when an inner IP header was available, the addresses) to be the
/// probe's own. The DGRAM error queue passes `addrs: None` because it hands back only the
/// 8-byte header we built, with no inner IP header (Task 12, deviation 26).
///
/// Only the *first* of the three candidate sequence numbers that names a live probe is
/// validated; if it does not check out, the reply is dropped rather than the next candidate
/// tried. This is faithful to C's `handle_inner_udp_packet()` (deconstruct_unix.c:63-121),
/// which builds one `probe` from the same ordered guesses and then returns on the first
/// mismatch. A fixed `dest-port` that happens to equal another outstanding probe's sequence
/// therefore shadows the true match in C exactly as it does here.
pub fn match_udp(
    table: &ProbeTable,
    src_port: u16,
    dst_port: u16,
    checksum: u16,
    addrs: Option<(IpAddr, IpAddr)>,
) -> Option<usize> {
    let idx = [dst_port, src_port, checksum]
        .into_iter()
        .find_map(|s| table.find_by_sequence(s))?;
    let p = &table.probes[idx];
    if p.protocol != Protocol::Udp || dst_port != p.remote.port() || src_port != p.local.port() {
        return None;
    }
    match addrs {
        Some((src, dst)) if dst != p.remote.ip() || src != p.local.ip() => None,
        _ => Some(idx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe_table::ProbeTable;
    use mtr_proto::{MplsLabel, ProbeResult};
    use std::net::{IpAddr, SocketAddr};
    use std::time::Instant;

    fn ip4(proto: u8, src: [u8; 4], dst: [u8; 4], payload_len: usize) -> Vec<u8> {
        let mut h = vec![0x45, 0, 0, 0, 0, 0, 0x40, 0, 64, proto, 0, 0];
        let len = (20 + payload_len) as u16;
        h[2..4].copy_from_slice(&len.to_be_bytes());
        h.extend_from_slice(&src);
        h.extend_from_slice(&dst);
        h
    }
    /// Like `ip4()`, but with IPv4 options: `options` is padded to a multiple of 4 bytes and the
    /// IHL nibble and total length are set accordingly.
    fn ip4_opts(
        proto: u8,
        src: [u8; 4],
        dst: [u8; 4],
        payload_len: usize,
        options: &[u8],
    ) -> Vec<u8> {
        let mut opts = options.to_vec();
        while !opts.len().is_multiple_of(4) {
            opts.push(1); // NOP
        }
        let mut h = ip4(proto, src, dst, payload_len + opts.len());
        h[0] = 0x40 | ((20 + opts.len()) / 4) as u8;
        h.extend_from_slice(&opts);
        h
    }
    fn icmp(t: u8, code: u8, id: u16, seq: u16) -> Vec<u8> {
        let mut v = vec![t, code, 0, 0];
        v.extend_from_slice(&id.to_be_bytes());
        v.extend_from_slice(&seq.to_be_bytes());
        v
    }
    fn udp(sp: u16, dp: u16, csum: u16) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&sp.to_be_bytes());
        v.extend_from_slice(&dp.to_be_bytes());
        v.extend_from_slice(&12u16.to_be_bytes());
        v.extend_from_slice(&csum.to_be_bytes());
        v.extend_from_slice(&[0; 4]);
        v
    }

    #[test]
    fn ipv4_options_are_skipped_in_outer_and_inner_headers() {
        // Outer: echo reply behind a 24-byte header (one NOP-padded option word).
        let mut pkt = ip4_opts(IPPROTO_ICMP, [10, 0, 0, 1], [10, 0, 0, 2], 8, &[1, 1, 1, 1]);
        pkt.extend(icmp(0, 0, 0x1234, 7));
        let p = parse_icmp4(&pkt, true).unwrap();
        assert_eq!(p.kind, IcmpKind::EchoReply);
        assert_eq!(p.echo, Some((0x1234, 7)));

        // IHL 15 is the maximum valid IPv4 header: 20 bytes plus 40 bytes of options.
        let mut max = ip4_opts(IPPROTO_ICMP, [10, 0, 0, 1], [10, 0, 0, 2], 8, &[1; 40]);
        max.extend(icmp(0, 0, 0x4321, 9));
        let p = parse_icmp4(&max, true).unwrap();
        assert_eq!(p.kind, IcmpKind::EchoReply);
        assert_eq!(p.echo, Some((0x4321, 9)));

        // Inner: time-exceeded quoting a UDP probe whose IP header carries options.
        let mut quoted = ip4_opts(IPPROTO_UDP, [10, 0, 0, 2], [10, 0, 0, 9], 12, &[1, 1, 1, 1]);
        quoted.extend(udp(33000, 33434, 0));
        let mut outer = ip4(IPPROTO_ICMP, [10, 0, 0, 5], [10, 0, 0, 2], 8 + quoted.len());
        outer.extend(icmp(11, 0, 0, 0));
        outer.extend(&quoted);
        let p = parse_icmp4(&outer, true).unwrap();
        assert_eq!(p.kind, IcmpKind::TimeExceeded);
        assert_eq!(
            p.inner,
            Some(Inner::Udp {
                src: "10.0.0.2".parse().unwrap(),
                dst: "10.0.0.9".parse().unwrap(),
                src_port: 33000,
                dst_port: 33434,
                checksum: 0,
            })
        );
    }

    #[test]
    fn bogus_ihl_is_rejected() {
        let mut pkt = ip4(IPPROTO_ICMP, [10, 0, 0, 1], [10, 0, 0, 2], 8);
        pkt.extend(icmp(0, 0, 1, 1));
        pkt[0] = 0x44; // IHL 4 (< 5)
        assert!(parse_icmp4(&pkt, true).is_none());
        pkt[0] = 0x4f; // IHL 15 = 60 bytes > packet
        assert!(parse_icmp4(&pkt, true).is_none());
        pkt[0] = 0x64; // version 6 in an IPv4 packet
        assert!(parse_icmp4(&pkt, true).is_none());
    }

    #[test]
    fn be16_is_none_past_the_end() {
        assert_eq!(be16(&[1, 2, 3], 1), Some(0x0203));
        assert_eq!(be16(&[1, 2, 3], 2), None);
        assert_eq!(be16(&[], 0), None);
    }

    #[test]
    fn echo_reply_over_a_raw_socket_carries_the_ip_header() {
        let mut pkt = ip4(1, [8, 8, 8, 8], [10, 0, 0, 2], 8);
        pkt.extend(icmp(0, 0, 0x1234, 33434));
        let p = parse_icmp4(&pkt, true).unwrap();
        assert_eq!(p.kind, IcmpKind::EchoReply);
        assert_eq!(p.echo, Some((0x1234, 33434)));
        assert!(p.inner.is_none() && p.mpls.is_empty());
        // DGRAM sockets deliver the ICMP header first.
        let p = parse_icmp4(&icmp(0, 0, 0x1234, 33434), false).unwrap();
        assert_eq!(p.echo, Some((0x1234, 33434)));
        assert!(parse_icmp4(&pkt[..25], true).is_none(), "too short");
        let mut not_icmp = ip4(17, [8, 8, 8, 8], [10, 0, 0, 2], 8);
        not_icmp.extend(icmp(0, 0, 1, 1));
        assert!(parse_icmp4(&not_icmp, true).is_none());
    }

    #[test]
    fn time_exceeded_exposes_the_inner_icmp_udp_and_tcp_headers() {
        let inner_icmp = {
            let mut v = ip4(1, [10, 0, 0, 2], [8, 8, 8, 8], 8);
            v.extend(icmp(8, 0, 0x1234, 33435));
            v
        };
        let mut pkt = ip4(1, [10, 0, 0, 1], [10, 0, 0, 2], 8 + inner_icmp.len());
        pkt.extend(icmp(11, 0, 0, 0));
        pkt.extend(&inner_icmp);
        let p = parse_icmp4(&pkt, true).unwrap();
        assert_eq!(p.kind, IcmpKind::TimeExceeded);
        assert_eq!(
            p.inner,
            Some(Inner::Icmp {
                id: 0x1234,
                sequence: 33435
            })
        );

        let mut pkt = ip4(1, [10, 0, 0, 1], [10, 0, 0, 2], 8 + 20 + 12);
        pkt.extend(icmp(3, 3, 0, 0)); // port unreachable
        pkt.extend(ip4(17, [10, 0, 0, 2], [8, 8, 8, 8], 12));
        pkt.extend(udp(5555, 33436, 0));
        let p = parse_icmp4(&pkt, true).unwrap();
        assert_eq!(p.kind, IcmpKind::DestUnreach { code: 3 });
        assert_eq!(
            p.inner,
            Some(Inner::Udp {
                src: "10.0.0.2".parse().unwrap(),
                dst: "8.8.8.8".parse().unwrap(),
                src_port: 5555,
                dst_port: 33436,
                checksum: 0
            })
        );

        let mut pkt = ip4(1, [10, 0, 0, 1], [10, 0, 0, 2], 8 + 20 + 8);
        pkt.extend(icmp(11, 0, 0, 0));
        pkt.extend(ip4(6, [10, 0, 0, 2], [8, 8, 8, 8], 8));
        pkt.extend(33437u16.to_be_bytes());
        pkt.extend([0, 80, 0, 0, 0, 0]);
        assert_eq!(
            parse_icmp4(&pkt, true).unwrap().inner,
            Some(Inner::Stream { src_port: 33437 })
        );
    }

    #[test]
    fn icmpv6_time_exceeded_with_inner_udp() {
        let mut pkt = icmp(3, 0, 0, 0); // ICMP6_TIME_EXCEEDED
        let mut ip6 = vec![0x60, 0, 0, 0, 0, 12, 17, 64];
        ip6.extend_from_slice(
            &"2001:db8::2"
                .parse::<std::net::Ipv6Addr>()
                .unwrap()
                .octets(),
        );
        ip6.extend_from_slice(
            &"2001:db8::9"
                .parse::<std::net::Ipv6Addr>()
                .unwrap()
                .octets(),
        );
        pkt.extend(ip6);
        pkt.extend(udp(40000, 164, 0));
        let p = parse_icmp6(&pkt).unwrap();
        assert_eq!(p.kind, IcmpKind::TimeExceeded);
        assert!(matches!(
            p.inner,
            Some(Inner::Udp {
                src_port: 40000,
                dst_port: 164,
                ..
            })
        ));
        let mut echo = icmp(129, 0, 7, 33438);
        echo.extend_from_slice(&[0; 8]);
        assert_eq!(parse_icmp6(&echo).unwrap().echo, Some((7, 33438)));
        let mut unreach = icmp(1, 4, 0, 0);
        unreach.extend(vec![0u8; 60]);
        assert_eq!(
            parse_icmp6(&unreach).unwrap().kind,
            IcmpKind::DestUnreach { code: 4 }
        );
    }

    #[test]
    fn mpls_labels_are_decoded_from_the_rfc_4884_extension() {
        // ICMP header + 128-byte original datagram + ext header + one MPLS object with two labels
        let mut icmp_bytes = icmp(11, 0, 0, 0);
        let mut inner = ip4(1, [10, 0, 0, 2], [8, 8, 8, 8], 8);
        inner.extend(icmp(8, 0, 0x1234, 33435));
        inner.resize(128, 0);
        icmp_bytes.extend(inner);
        icmp_bytes.extend_from_slice(&[0x20, 0, 0, 0]); // version 2
        icmp_bytes.extend_from_slice(&[0, 12, 1, 1]); // len 12, class 1, ctype 1
        icmp_bytes.extend_from_slice(&[0x00, 0x01, 0x01, 0xff]); // label 16, tc 0, s 1, ttl 255
        icmp_bytes.extend_from_slice(&[0x03, 0xe8, 0x06, 0x01]); // label 16000, tc 3, s 0, ttl 1
        let labels = decode_mpls(&icmp_bytes);
        assert_eq!(
            labels,
            vec![
                MplsLabel {
                    label: 16,
                    tc: 0,
                    bottom_of_stack: true,
                    ttl: 255
                },
                MplsLabel {
                    label: 16000,
                    tc: 3,
                    bottom_of_stack: false,
                    ttl: 1
                },
            ]
        );
        let mut pkt = ip4(1, [10, 0, 0, 1], [10, 0, 0, 2], icmp_bytes.len());
        pkt.extend(&icmp_bytes);
        let p = parse_icmp4(&pkt, true).unwrap();
        assert_eq!(p.mpls.len(), 2);
        assert_eq!(
            p.inner,
            Some(Inner::Icmp {
                id: 0x1234,
                sequence: 33435
            })
        );
        // Wrong version nibble → no labels; short packet → no labels.
        let mut bad = icmp_bytes.clone();
        bad[8 + 128] = 0x10;
        assert!(decode_mpls(&bad).is_empty());
        assert!(decode_mpls(&icmp_bytes[..100]).is_empty());
    }

    fn table_with(
        seq: u16,
        protocol: mtr_proto::Protocol,
        local: &str,
        remote: &str,
    ) -> ProbeTable {
        let mut t = ProbeTable::new();
        let i = t.alloc(1, Instant::now(), 10).unwrap();
        t.probes[i].sequence = seq;
        t.probes[i].protocol = protocol;
        t.probes[i].local = local.parse::<SocketAddr>().unwrap();
        t.probes[i].remote = remote.parse::<SocketAddr>().unwrap();
        t
    }

    #[test]
    fn matching_follows_find_probe_and_handle_inner_rules() {
        let t = table_with(33434, mtr_proto::Protocol::Icmp, "10.0.0.2:0", "8.8.8.8:0");
        let echo = Parsed {
            kind: IcmpKind::EchoReply,
            echo: Some((0x1234, 33434)),
            inner: None,
            mpls: vec![],
        };
        assert_eq!(
            match_reply(&t, &echo, 4, 0x1234),
            Some((0, ProbeResult::Reply))
        );
        assert_eq!(match_reply(&t, &echo, 4, 0x9999), None, "foreign ICMP id");
        let exp = Parsed {
            kind: IcmpKind::TimeExceeded,
            echo: None,
            inner: Some(Inner::Icmp {
                id: 0x1234,
                sequence: 33434,
            }),
            mpls: vec![],
        };
        assert_eq!(
            match_reply(&t, &exp, 4, 0x1234),
            Some((0, ProbeResult::TtlExpired))
        );

        let t = table_with(
            33436,
            mtr_proto::Protocol::Udp,
            "10.0.0.2:5555",
            "8.8.8.8:33436",
        );
        let udp_inner =
            |dst_port: u16, src_port: u16, checksum: u16, src: &str, dst: &str| Inner::Udp {
                src: src.parse::<IpAddr>().unwrap(),
                dst: dst.parse().unwrap(),
                src_port,
                dst_port,
                checksum,
            };
        let ok = Parsed {
            kind: IcmpKind::DestUnreach { code: 3 },
            echo: None,
            inner: Some(udp_inner(33436, 5555, 0, "10.0.0.2", "8.8.8.8")),
            mpls: vec![],
        };
        assert_eq!(match_reply(&t, &ok, 4, 1), Some((0, ProbeResult::Reply)));
        let other_code = Parsed {
            kind: IcmpKind::DestUnreach { code: 1 },
            ..ok.clone()
        };
        assert_eq!(
            match_reply(&t, &other_code, 4, 1),
            Some((0, ProbeResult::NoRouteHost))
        );
        let wrong_src_port = Parsed {
            inner: Some(udp_inner(33436, 4444, 0, "10.0.0.2", "8.8.8.8")),
            ..ok.clone()
        };
        assert_eq!(match_reply(&t, &wrong_src_port, 4, 1), None);
        let wrong_addr = Parsed {
            inner: Some(udp_inner(33436, 5555, 0, "10.0.0.9", "8.8.8.8")),
            ..ok.clone()
        };
        assert_eq!(match_reply(&t, &wrong_addr, 4, 1), None);
        // sequence in the checksum field (both ports requested)
        let t = table_with(
            33440,
            mtr_proto::Protocol::Udp,
            "10.0.0.2:1991",
            "8.8.8.8:990",
        );
        let by_csum = Parsed {
            kind: IcmpKind::TimeExceeded,
            echo: None,
            inner: Some(udp_inner(990, 1991, 33440, "10.0.0.2", "8.8.8.8")),
            mpls: vec![],
        };
        assert_eq!(
            match_reply(&t, &by_csum, 4, 1),
            Some((0, ProbeResult::TtlExpired))
        );

        let t = table_with(
            33437,
            mtr_proto::Protocol::Tcp,
            "10.0.0.2:33437",
            "8.8.8.8:80",
        );
        let tcp = Parsed {
            kind: IcmpKind::TimeExceeded,
            echo: None,
            inner: Some(Inner::Stream { src_port: 33437 }),
            mpls: vec![],
        };
        assert_eq!(
            match_reply(&t, &tcp, 4, 1),
            Some((0, ProbeResult::TtlExpired))
        );
    }

    /// Malformed input must never panic: every prefix of a well-formed packet, and pure
    /// garbage, has to fall out as `None`/empty rather than indexing past the buffer.
    #[test]
    fn truncated_and_garbage_buffers_are_rejected_without_panicking() {
        let mut mpls_icmp = icmp(11, 0, 0, 0);
        let mut inner = ip4(1, [10, 0, 0, 2], [8, 8, 8, 8], 8);
        inner.extend(icmp(8, 0, 0x1234, 33435));
        inner.resize(128, 0);
        mpls_icmp.extend(inner);
        mpls_icmp.extend_from_slice(&[0x20, 0, 0, 0]);
        mpls_icmp.extend_from_slice(&[0, 12, 1, 1]);
        mpls_icmp.extend_from_slice(&[0x00, 0x01, 0x01, 0xff]);
        mpls_icmp.extend_from_slice(&[0x03, 0xe8, 0x06, 0x01]);
        let mut v6 = icmp(3, 0, 0, 0);
        v6.extend(vec![0x60, 0, 0, 0, 0, 12, 17, 64]);
        v6.extend(vec![0u8; 32]);
        v6.extend(udp(40000, 164, 0));
        let mut raw4 = ip4(1, [10, 0, 0, 1], [10, 0, 0, 2], mpls_icmp.len());
        raw4.extend(&mpls_icmp);
        // A byte pattern that hits every protocol/type dispatch as it slides through the offsets.
        let garbage: Vec<u8> = (0u32..300)
            .map(|i| (i.wrapping_mul(37) ^ 0x5a) as u8)
            .collect();

        for buf in [&mpls_icmp, &v6, &raw4, &garbage] {
            for n in 0..=buf.len() {
                let b = &buf[..n];
                let _ = parse_icmp4(b, true);
                let _ = parse_icmp4(b, false);
                let _ = parse_icmp6(b);
                let _ = decode_mpls(b);
            }
        }

        // An object chain: a zero-length object aborts the walk (deconstruct_unix.c:381-383),
        // an over-long one too (:378-380), and a non-MPLS object is skipped (:391-392).
        let ext = |objs: &[u8]| {
            let mut v = icmp(11, 0, 0, 0);
            v.resize(ICMP_HEADER + ICMP_ORIGINAL_DATAGRAM_MIN_SIZE, 0);
            v.extend_from_slice(&[0x20, 0, 0, 0]);
            v.extend_from_slice(objs);
            v
        };
        assert!(
            decode_mpls(&ext(&[0, 0, 1, 1, 0, 8, 1, 1])).is_empty(),
            "len 0"
        );
        assert!(
            decode_mpls(&ext(&[0, 99, 1, 1])).is_empty(),
            "len past the end"
        );
        assert!(
            decode_mpls(&ext(&[0, 4, 2, 1])).is_empty(),
            "no MPLS object"
        );
        assert_eq!(
            decode_mpls(&ext(&[0, 4, 2, 1, 0, 8, 1, 1, 0x00, 0x01, 0x01, 0x40])),
            vec![MplsLabel {
                label: 16,
                tc: 0,
                bottom_of_stack: true,
                ttl: 0x40
            }],
            "the MPLS object after a skipped one"
        );
        // MAX_MPLS_LABELS caps the list even when the object carries more.
        let mut many = vec![0u8, 4 + 4 * 12, 1, 1];
        for i in 0..12u8 {
            many.extend_from_slice(&[0, 0, 0x01, i]);
        }
        assert_eq!(decode_mpls(&ext(&many)).len(), MAX_MPLS_LABELS);
    }
}
