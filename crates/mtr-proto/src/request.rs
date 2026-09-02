//! `check-support` / `send-probe` requests. Ported from packet/command.c (decoding) and
//! ui/cmdpipe.c (encoding order) — mtr 0.96, commit 7b01773. GPL-2.0-only.

use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::ParseError;
use crate::tokenize::{Line, tokenize};

/// Transport used for probes (`protocol` argument).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Protocol {
    #[default]
    Icmp,
    Udp,
    Tcp,
    Sctp,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Icmp => "icmp",
            Protocol::Udp => "udp",
            Protocol::Tcp => "tcp",
            Protocol::Sctp => "sctp",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "icmp" => Some(Protocol::Icmp),
            "udp" => Some(Protocol::Udp),
            "tcp" => Some(Protocol::Tcp),
            "sctp" => Some(Protocol::Sctp),
            _ => None,
        }
    }
}

/// Feature names accepted by `check-support` (command.c:93-156).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    Version,
    Ip4,
    Ip6,
    SendProbe,
    Icmp,
    Udp,
    Tcp,
    Sctp,
    Mark,
}

impl Feature {
    pub fn as_str(self) -> &'static str {
        match self {
            Feature::Version => "version",
            Feature::Ip4 => "ip-4",
            Feature::Ip6 => "ip-6",
            Feature::SendProbe => "send-probe",
            Feature::Icmp => "icmp",
            Feature::Udp => "udp",
            Feature::Tcp => "tcp",
            Feature::Sctp => "sctp",
            Feature::Mark => "mark",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "version" => Some(Feature::Version),
            "ip-4" => Some(Feature::Ip4),
            "ip-6" => Some(Feature::Ip6),
            "send-probe" => Some(Feature::SendProbe),
            "icmp" => Some(Feature::Icmp),
            "udp" => Some(Feature::Udp),
            "tcp" => Some(Feature::Tcp),
            "sctp" => Some(Feature::Sctp),
            "mark" => Some(Feature::Mark),
            _ => None,
        }
    }

    pub fn for_protocol(p: Protocol) -> Feature {
        match p {
            Protocol::Icmp => Feature::Icmp,
            Protocol::Udp => Feature::Udp,
            Protocol::Tcp => Feature::Tcp,
            Protocol::Sctp => Feature::Sctp,
        }
    }
}

/// Arguments of `send-probe`. `None` means "omit"; the helper then uses its defaults
/// (protocol icmp, ttl 255, size 64, timeout 10 — command.c:330-336).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeParams {
    pub target: IpAddr,
    pub local_ip: Option<IpAddr>,
    pub local_device: Option<String>,
    pub protocol: Protocol,
    pub size: Option<u16>,
    pub bit_pattern: Option<u8>,
    pub tos: Option<u8>,
    pub ttl: Option<u8>,
    pub timeout_s: Option<u32>,
    pub port: Option<u16>,
    pub local_port: Option<u16>,
    pub mark: Option<u32>,
}

impl ProbeParams {
    pub fn new(target: IpAddr) -> Self {
        ProbeParams {
            target,
            local_ip: None,
            local_device: None,
            protocol: Protocol::Icmp,
            size: None,
            bit_pattern: None,
            tos: None,
            ttl: None,
            timeout_s: None,
            port: None,
            local_port: None,
            mark: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestKind {
    CheckSupport { feature: Feature },
    SendProbe(ProbeParams),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub token: i32,
    pub kind: RequestKind,
}

fn ip_key(ip: IpAddr, v4: &'static str, v6: &'static str) -> &'static str {
    if ip.is_ipv4() { v4 } else { v6 }
}

fn bad(name: &'static str, value: &str) -> ParseError {
    ParseError::InvalidValue {
        name,
        value: value.to_string(),
    }
}

fn num<T: TryFrom<i64>>(l: &Line<'_>, name: &'static str) -> Result<Option<T>, ParseError> {
    match l.last(name) {
        None => Ok(None),
        Some(v) => {
            let n: i64 = v.parse().map_err(|_| bad(name, v))?;
            T::try_from(n).map(Some).map_err(|_| bad(name, v))
        }
    }
}

impl Request {
    /// Encode as one protocol line including the trailing newline, in the field order
    /// `construct_base_command()` + `send_probe_command()` use (cmdpipe.c:387-551).
    pub fn encode(&self) -> String {
        let mut s = String::with_capacity(160);
        match &self.kind {
            RequestKind::CheckSupport { feature } => {
                let _ = write!(
                    s,
                    "{} check-support feature {}",
                    self.token,
                    feature.as_str()
                );
            }
            RequestKind::SendProbe(p) => {
                let _ = write!(
                    s,
                    "{} send-probe {} {}",
                    self.token,
                    ip_key(p.target, "ip-4", "ip-6"),
                    p.target
                );
                if let Some(l) = p.local_ip {
                    let _ = write!(s, " {} {}", ip_key(l, "local-ip-4", "local-ip-6"), l);
                }
                let _ = write!(s, " protocol {}", p.protocol.as_str());
                if let Some(v) = p.size {
                    let _ = write!(s, " size {v}");
                }
                if let Some(v) = p.bit_pattern {
                    let _ = write!(s, " bit-pattern {v}");
                }
                if let Some(v) = p.tos {
                    let _ = write!(s, " tos {v}");
                }
                if let Some(v) = p.ttl {
                    let _ = write!(s, " ttl {v}");
                }
                if let Some(v) = p.timeout_s {
                    let _ = write!(s, " timeout {v}");
                }
                if let Some(v) = p.port {
                    let _ = write!(s, " port {v}");
                }
                if let Some(v) = p.local_port {
                    let _ = write!(s, " local-port {v}");
                }
                if let Some(v) = p.mark {
                    let _ = write!(s, " mark {v}");
                }
                if let Some(d) = &p.local_device {
                    let _ = write!(s, " local-device {d}");
                }
            }
        }
        s.push('\n');
        s
    }

    /// Parse one line (trailing newline optional).
    pub fn parse(line: &str) -> Result<Request, ParseError> {
        let l = tokenize(line)?;
        let kind = match l.name {
            "check-support" => {
                let f = l
                    .first("feature")
                    .ok_or(ParseError::MissingArgument("feature"))?;
                let feature =
                    Feature::parse(f).ok_or_else(|| ParseError::UnknownFeature(f.to_string()))?;
                RequestKind::CheckSupport { feature }
            }
            "send-probe" => RequestKind::SendProbe(parse_probe_params(&l)?),
            other => return Err(ParseError::UnknownCommand(other.to_string())),
        };
        Ok(Request {
            token: l.token,
            kind,
        })
    }
}

fn parse_probe_params(l: &Line<'_>) -> Result<ProbeParams, ParseError> {
    // command.c:338-346 decodes arguments in order, so the last ip-4/ip-6 wins and
    // local-ip-* is parsed in the family selected by the target.
    let mut target: Option<IpAddr> = None;
    let mut local: Option<&str> = None;
    for (k, v) in &l.args {
        match *k {
            "ip-4" => target = Some(v.parse::<Ipv4Addr>().map_err(|_| bad("ip-4", v))?.into()),
            "ip-6" => target = Some(v.parse::<Ipv6Addr>().map_err(|_| bad("ip-6", v))?.into()),
            "local-ip-4" | "local-ip-6" => local = Some(v),
            _ => {}
        }
    }
    let target = target.ok_or(ParseError::MissingArgument("ip-4"))?;
    let local_ip = match local {
        None => None,
        Some(v) if target.is_ipv4() => Some(
            v.parse::<Ipv4Addr>()
                .map_err(|_| bad("local-ip-4", v))?
                .into(),
        ),
        Some(v) => Some(
            v.parse::<Ipv6Addr>()
                .map_err(|_| bad("local-ip-6", v))?
                .into(),
        ),
    };
    let protocol = match l.last("protocol") {
        None => Protocol::Icmp,
        Some(p) => Protocol::parse(p).ok_or_else(|| bad("protocol", p))?,
    };
    let bit_pattern = match l.last("bit-pattern") {
        None => None,
        Some(v) => {
            let n: i64 = v.parse().map_err(|_| bad("bit-pattern", v))?;
            Some((n & 0xff) as u8) // memset() keeps the low byte (construct_unix.c:852)
        }
    };
    Ok(ProbeParams {
        target,
        local_ip,
        local_device: l.last("local-device").map(str::to_string),
        protocol,
        size: num::<u16>(l, "size")?,
        bit_pattern,
        tos: num::<u8>(l, "tos")?,
        ttl: num::<u8>(l, "ttl")?,
        timeout_s: num::<u32>(l, "timeout")?,
        port: num::<u16>(l, "port")?,
        local_port: num::<u16>(l, "local-port")?,
        mark: num::<u32>(l, "mark")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ParseError;
    use std::net::IpAddr;

    fn v4(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn encodes_check_support_like_cmdpipe() {
        let r = Request {
            token: 1,
            kind: RequestKind::CheckSupport {
                feature: Feature::SendProbe,
            },
        };
        assert_eq!(r.encode(), "1 check-support feature send-probe\n");
    }

    #[test]
    fn encodes_send_probe_in_cmdpipe_field_order() {
        let mut p = ProbeParams::new(v4("192.0.2.1"));
        p.local_ip = Some(v4("192.0.2.100"));
        p.size = Some(64);
        p.bit_pattern = Some(0);
        p.tos = Some(0);
        p.ttl = Some(1);
        p.timeout_s = Some(10);
        p.port = Some(80);
        p.local_port = Some(33001);
        p.mark = Some(7);
        p.local_device = Some("eth0".into());
        let r = Request {
            token: 33000,
            kind: RequestKind::SendProbe(p),
        };
        assert_eq!(
            r.encode(),
            "33000 send-probe ip-4 192.0.2.1 local-ip-4 192.0.2.100 protocol icmp size 64 bit-pattern 0 tos 0 ttl 1 timeout 10 port 80 local-port 33001 mark 7 local-device eth0\n"
        );
    }

    #[test]
    fn encodes_ipv6_keys_and_omits_absent_fields() {
        let mut p = ProbeParams::new("2001:db8::1".parse().unwrap());
        p.local_ip = Some("2001:db8::2".parse().unwrap());
        p.protocol = Protocol::Udp;
        let r = Request {
            token: 5,
            kind: RequestKind::SendProbe(p),
        };
        assert_eq!(
            r.encode(),
            "5 send-probe ip-6 2001:db8::1 local-ip-6 2001:db8::2 protocol udp\n"
        );
    }

    #[test]
    fn parses_send_probe_with_defaults_and_ignores_unknown_args() {
        let r = Request::parse("42 send-probe ip-4 10.0.0.1 ttl 3 frobnicate yes\n").unwrap();
        assert_eq!(r.token, 42);
        let RequestKind::SendProbe(p) = r.kind else {
            panic!("not a probe")
        };
        assert_eq!(p.target, v4("10.0.0.1"));
        assert_eq!(p.ttl, Some(3));
        assert_eq!(p.protocol, Protocol::Icmp);
        assert_eq!(p.size, None);
        assert_eq!(p.local_ip, None);
    }

    #[test]
    fn negative_bit_pattern_is_truncated_like_memset() {
        let r = Request::parse("1 send-probe ip-4 10.0.0.1 bit-pattern -257").unwrap();
        let RequestKind::SendProbe(p) = r.kind else {
            panic!("not a probe")
        };
        assert_eq!(p.bit_pattern, Some(255));
    }

    #[test]
    fn last_duplicate_wins_for_send_probe_first_for_check_support() {
        let r = Request::parse("1 send-probe ip-4 10.0.0.1 ttl 3 ttl 9").unwrap();
        let RequestKind::SendProbe(p) = r.kind else {
            panic!("not a probe")
        };
        assert_eq!(p.ttl, Some(9));
        let r = Request::parse("1 check-support feature udp feature tcp").unwrap();
        assert_eq!(
            r.kind,
            RequestKind::CheckSupport {
                feature: Feature::Udp
            }
        );
    }

    #[test]
    fn local_ip_family_follows_target() {
        let r = Request::parse("1 send-probe ip-6 2001:db8::9 local-ip-6 2001:db8::1").unwrap();
        let RequestKind::SendProbe(p) = r.kind else {
            panic!("not a probe")
        };
        assert_eq!(p.local_ip, Some("2001:db8::1".parse().unwrap()));
        assert!(Request::parse("1 send-probe ip-4 10.0.0.1 local-ip-4 zzz").is_err());
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(
            Request::parse("1 send-probe ttl 3"),
            Err(ParseError::MissingArgument("ip-4"))
        );
        assert_eq!(
            Request::parse("1 send-probe ip-4 10.0.0.1 port 70000"),
            Err(ParseError::InvalidValue {
                name: "port",
                value: "70000".into()
            })
        );
        assert_eq!(
            Request::parse("1 send-probe ip-4 10.0.0.1 protocol gre"),
            Err(ParseError::InvalidValue {
                name: "protocol",
                value: "gre".into()
            })
        );
        assert_eq!(
            Request::parse("1 frobnicate a b"),
            Err(ParseError::UnknownCommand("frobnicate".into()))
        );
        assert_eq!(
            Request::parse("1 check-support"),
            Err(ParseError::MissingArgument("feature"))
        );
        assert_eq!(
            Request::parse("1 check-support feature warp"),
            Err(ParseError::UnknownFeature("warp".into()))
        );
    }

    #[test]
    fn round_trips() {
        let line = "33000 send-probe ip-4 192.0.2.1 local-ip-4 192.0.2.100 protocol tcp size 64 bit-pattern 0 tos 0 ttl 1 timeout 10 port 443\n";
        assert_eq!(Request::parse(line).unwrap().encode(), line);
    }
}
