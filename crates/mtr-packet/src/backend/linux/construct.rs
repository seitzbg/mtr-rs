//! Probe packet construction. Ported from packet/construct_unix.c:40-295, 482-548 (mtr 0.96,
//! commit 7b01773) — the IPv4 header is never built (no IP_HDRINCL; the kernel adds it), only
//! accounted for in the size. GPL-2.0-only.

use std::net::IpAddr;

use mtr_proto::{CProbeParams, Protocol};

pub const IP4_HEADER: usize = 20;
pub const IP6_HEADER: usize = 40;
pub const ICMP_HEADER: usize = 8;
pub const UDP_HEADER: usize = 8;
pub const ICMP_ECHO: u8 = 8;
pub const ICMP6_ECHO: u8 = 128;
const MIN_UNPRIVILEGED_PORT: u32 = 1024;
const UDP_PORT_RANGE: u32 = 65536;

/// `compute_checksum()` (construct_unix.c:58-89): 16-bit one's-complement sum, odd trailing
/// byte in the high half.
pub fn checksum(bytes: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for (i, b) in bytes.iter().enumerate() {
        sum += if i % 2 == 0 {
            u32::from(*b) << 8
        } else {
            u32::from(*b)
        };
    }
    while sum >> 16 != 0 {
        sum = (sum >> 16) + (sum & 0xffff);
    }
    !(sum as u16)
}

/// `compute_packet_size()` (construct_unix.c:489-548).
pub fn packet_size(params: &CProbeParams, is_raw: bool) -> Option<usize> {
    if matches!(params.protocol, Protocol::Tcp | Protocol::Sctp) {
        return Some(0);
    }
    let mut size = match params.ip_version {
        6 if is_raw => IP6_HEADER,
        4 if is_raw => IP4_HEADER,
        4 | 6 => 0,
        _ => return None,
    };
    size += match params.protocol {
        Protocol::Icmp => ICMP_HEADER,
        Protocol::Udp => UDP_HEADER + 4, // room for the sequence in the payload
        _ => return None,
    };
    if let Ok(requested) = usize::try_from(params.packet_size) {
        size = size.max(requested);
    }
    if params.ip_version == 6 && is_raw {
        size -= IP6_HEADER; // the kernel prepends it
    }
    Some(size)
}

/// `construct_icmp4_packet()` / `construct_icmp6_packet()` over an already-filled buffer
/// (construct_unix.c:104-123, :832-872).
pub fn icmp_echo(buf: &mut [u8], version: u8, id: u16, sequence: u16) {
    buf[..ICMP_HEADER].fill(0);
    buf[0] = if version == 6 { ICMP6_ECHO } else { ICMP_ECHO };
    buf[4..6].copy_from_slice(&id.to_be_bytes());
    buf[6..8].copy_from_slice(&sequence.to_be_bytes());
    if version == 4 {
        let c = checksum(buf);
        buf[2..4].copy_from_slice(&c.to_be_bytes());
    }
}

/// `udp_source_port_from_pid()` (construct_unix.c:47-56).
pub fn udp_source_port_from_pid(pid: u32) -> u16 {
    let mut port = pid & 0xffff;
    if port < MIN_UNPRIVILEGED_PORT {
        port += UDP_PORT_RANGE - MIN_UNPRIVILEGED_PORT;
    }
    port as u16
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UdpPorts {
    pub src: u16,
    pub dst: u16,
    pub checksum_is_sequence: bool,
}

/// `set_udp_ports()`: where the sequence number travels depends on which ports were requested
/// (construct_unix.c:157-185).
pub fn udp_ports(params: &CProbeParams, sequence: u16, pid_port: u16) -> UdpPorts {
    let dest = params.dest_port as u16;
    let local = params.local_port as u16;
    if params.dest_port != 0 {
        if params.local_port != 0 {
            UdpPorts {
                src: local,
                dst: dest,
                checksum_is_sequence: true,
            }
        } else {
            UdpPorts {
                src: sequence,
                dst: dest,
                checksum_is_sequence: false,
            }
        }
    } else {
        let src = if params.local_port != 0 {
            local
        } else {
            pid_port
        };
        UdpPorts {
            src,
            dst: sequence,
            checksum_is_sequence: false,
        }
    }
}

/// `UDPPseudoHeader` / `IP6PseudoHeader` bytes for `udp_len` bytes of UDP.
pub fn pseudo_header(src: IpAddr, dst: IpAddr, udp_len: u16) -> Vec<u8> {
    let mut v = Vec::with_capacity(40);
    match (src, dst) {
        (IpAddr::V4(s), IpAddr::V4(d)) => {
            v.extend_from_slice(&s.octets());
            v.extend_from_slice(&d.octets());
            v.push(0);
            v.push(17);
            v.extend_from_slice(&udp_len.to_be_bytes());
        }
        (IpAddr::V6(s), IpAddr::V6(d)) => {
            v.extend_from_slice(&s.octets());
            v.extend_from_slice(&d.octets());
            v.extend_from_slice(&u32::from(udp_len).to_be_bytes());
            v.extend_from_slice(&[0, 0, 0, 17]);
        }
        _ => {}
    }
    v
}

/// `construct_udp4_packet()` / `construct_udp6_packet()`: header, length, and a checksum that
/// either sits in the checksum field or — when that field carries the sequence — is balanced
/// through the two payload bytes right after the header (construct_unix.c:187-253).
pub fn udp_datagram(buf: &mut [u8], ports: UdpPorts, sequence: u16, src: IpAddr, dst: IpAddr) {
    let len = buf.len() as u16;
    buf[..UDP_HEADER].fill(0);
    buf[0..2].copy_from_slice(&ports.src.to_be_bytes());
    buf[2..4].copy_from_slice(&ports.dst.to_be_bytes());
    buf[4..6].copy_from_slice(&len.to_be_bytes());
    if ports.checksum_is_sequence {
        buf[6..8].copy_from_slice(&sequence.to_be_bytes());
    }
    let mut all = pseudo_header(src, dst, len);
    all.extend_from_slice(buf);
    if ports.checksum_is_sequence && buf.len() >= UDP_HEADER + 2 {
        let off = all.len() - buf.len() + UDP_HEADER;
        all[off] = 0;
        all[off + 1] = 0;
        let c = checksum(&all);
        buf[UDP_HEADER..UDP_HEADER + 2].copy_from_slice(&c.to_be_bytes());
    } else {
        let c = checksum(&all);
        buf[6..8].copy_from_slice(&c.to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtr_proto::{CProbeParams, Protocol};
    use std::net::IpAddr;

    #[test]
    fn checksum_matches_rfc_1071_examples() {
        // 0x0800 + 0x1234 + 0x0001 = 0x1A35 → one's complement 0xE5CA
        assert_eq!(
            checksum(&[0x08, 0x00, 0x00, 0x00, 0x12, 0x34, 0x00, 0x01]),
            0xE5CA
        );
        assert_eq!(checksum(&[]), 0xFFFF);
        assert_eq!(checksum(&[0xFF, 0xFF]), 0x0000);
        assert_eq!(checksum(&[0x01]), !0x0100u16); // odd length: last byte is the high byte
    }

    #[test]
    fn sizes_follow_compute_packet_size() {
        // `CProbeParams::default()` leaves `ip_version` at 0 (unset — decode_send_probe only
        // assigns it when an `ip-4`/`ip-6` argument is present, cdecode.rs:103-110); a real
        // caller always supplies one, and this is exercised by the final `None` case below,
        // so set it explicitly here to test the "known IP version" path.
        let mut p = CProbeParams {
            ip_version: 4,
            ..Default::default()
        }; // icmp, size 64
        assert_eq!(packet_size(&p, true), Some(64)); // 20 + 8 < 64
        assert_eq!(packet_size(&p, false), Some(64));
        p.packet_size = 10;
        assert_eq!(packet_size(&p, true), Some(28)); // IP header counted even though the kernel adds it
        assert_eq!(packet_size(&p, false), Some(10)); // 8 (ICMP header) < 10 requested, so the request wins
        p.protocol = Protocol::Udp;
        assert_eq!(packet_size(&p, true), Some(20 + 8 + 4));
        assert_eq!(packet_size(&p, false), Some(12));
        p.ip_version = 6;
        p.packet_size = 100;
        assert_eq!(packet_size(&p, true), Some(60)); // v6 raw subtracts the 40-byte header
        assert_eq!(packet_size(&p, false), Some(100));
        p.protocol = Protocol::Tcp;
        assert_eq!(packet_size(&p, true), Some(0));
        p.protocol = Protocol::Icmp;
        p.ip_version = 0;
        assert_eq!(packet_size(&p, true), None);
    }

    #[test]
    fn icmp4_echo_is_type_8_with_a_checksum_over_the_whole_buffer() {
        let mut buf = vec![0u8; 64];
        icmp_echo(&mut buf, 4, 0x1234, 33434);
        // sum over the zero-checksum buffer = 0x0800 + 0x1234 + 0x829A (33434 = 0x829A) = 0x9CCE,
        // one's complement 0x6331; folding that back in makes the whole buffer sum to zero.
        assert_eq!(&buf[..8], &[8, 0, 0x63, 0x31, 0x12, 0x34, 0x82, 0x9A]);
        assert_eq!(checksum(&buf), 0);
        let mut buf6 = vec![0u8; 16];
        icmp_echo(&mut buf6, 6, 0x1234, 33434);
        assert_eq!(&buf6[..8], &[128, 0, 0, 0, 0x12, 0x34, 0x82, 0x9A]);
    }

    #[test]
    fn udp_ports_pick_where_the_sequence_lives() {
        let mut p = CProbeParams {
            protocol: Protocol::Udp,
            ..Default::default()
        };
        let u = udp_ports(&p, 40000, 5555);
        assert_eq!((u.src, u.dst, u.checksum_is_sequence), (5555, 40000, false));
        p.dest_port = 990;
        let u = udp_ports(&p, 40000, 5555);
        assert_eq!((u.src, u.dst, u.checksum_is_sequence), (40000, 990, false));
        p.local_port = 1991;
        let u = udp_ports(&p, 40000, 5555);
        assert_eq!((u.src, u.dst, u.checksum_is_sequence), (1991, 990, true));
        p.dest_port = 0;
        let u = udp_ports(&p, 40000, 5555);
        assert_eq!((u.src, u.dst, u.checksum_is_sequence), (1991, 40000, false));
        assert_eq!(udp_source_port_from_pid(70000), (70000u32 & 0xffff) as u16);
        assert_eq!(
            udp_source_port_from_pid(1000),
            (1000u32 + 65536 - 1024) as u16
        );
    }

    #[test]
    fn udp_datagrams_verify_against_the_pseudo_header_in_both_modes() {
        let src: IpAddr = "192.0.2.1".parse().unwrap();
        let dst: IpAddr = "192.0.2.2".parse().unwrap();
        let mut buf = vec![0x2Cu8; 40];
        udp_datagram(
            &mut buf,
            UdpPorts {
                src: 5555,
                dst: 40000,
                checksum_is_sequence: false,
            },
            40000,
            src,
            dst,
        );
        assert_eq!(u16::from_be_bytes([buf[4], buf[5]]), 40);
        let mut all = pseudo_header(src, dst, 40);
        all.extend_from_slice(&buf);
        assert_eq!(checksum(&all), 0, "checksum field balances the datagram");

        let mut buf = vec![0x2Cu8; 40];
        udp_datagram(
            &mut buf,
            UdpPorts {
                src: 1991,
                dst: 990,
                checksum_is_sequence: true,
            },
            40000,
            src,
            dst,
        );
        assert_eq!(
            u16::from_be_bytes([buf[6], buf[7]]),
            40000,
            "checksum field carries the sequence"
        );
        let mut all = pseudo_header(src, dst, 40);
        all.extend_from_slice(&buf);
        assert_eq!(
            checksum(&all),
            0,
            "payload bytes 8..10 were adjusted to balance"
        );

        let src6: IpAddr = "2001:db8::1".parse().unwrap();
        let dst6: IpAddr = "2001:db8::2".parse().unwrap();
        let mut buf = vec![0u8; 20];
        udp_datagram(
            &mut buf,
            UdpPorts {
                src: 5555,
                dst: 40000,
                checksum_is_sequence: false,
            },
            40000,
            src6,
            dst6,
        );
        let mut all = pseudo_header(src6, dst6, 20);
        all.extend_from_slice(&buf);
        assert_eq!(checksum(&all), 0);
    }
}
