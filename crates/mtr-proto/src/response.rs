//! Replies from `mtr-packet`. Vocabulary from packet/probe.c:250-320, packet/probe_unix.c:530-556,
//! packet/command.c and the client mapping in ui/cmdpipe.c:690-794 — mtr 0.96, commit 7b01773.
//! GPL-2.0-only.

use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::ParseError;
use crate::mpls::{MplsLabel, format_mpls_list, parse_mpls_list};
use crate::tokenize::{Line, tokenize};

/// Which ICMP outcome a probe reply reports (probe.c:265-273).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProbeResult {
    Reply,
    TtlExpired,
    NoRouteHost,
}

impl ProbeResult {
    pub fn as_str(self) -> &'static str {
        match self {
            ProbeResult::Reply => "reply",
            ProbeResult::TtlExpired => "ttl-expired",
            ProbeResult::NoRouteHost => "no-route-host",
        }
    }
}

/// `reason` values of `invalid-argument` (command.c:301-313).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InvalidReason {
    IpVersionNotSupported,
    ProtocolNotSupported,
}

impl InvalidReason {
    pub fn as_str(self) -> &'static str {
        match self {
            InvalidReason::IpVersionNotSupported => "ip-version-not-supported",
            InvalidReason::ProtocolNotSupported => "protocol-not-supported",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ip-version-not-supported" => Some(InvalidReason::IpVersionNotSupported),
            "protocol-not-supported" => Some(InvalidReason::ProtocolNotSupported),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseKind {
    /// `reply` / `ttl-expired` / `no-route-host` with an address and a round-trip time.
    Probe {
        result: ProbeResult,
        addr: IpAddr,
        rtt_us: u32,
        mpls: Vec<MplsLabel>,
    },
    /// Probe timed out inside the helper.
    NoReply,
    /// `feature-support support <value>`; value is `ok`, `no`, or the version string.
    FeatureSupport(String),
    InvalidArgument {
        reason: Option<InvalidReason>,
    },
    UnknownCommand,
    ProbesExhausted,
    PermissionDenied,
    AddressInUse,
    AddressNotAvailable,
    NetworkDown,
    HostDown,
    NoRouteNetwork,
    /// Bare errno-mapped form without address/RTT (probe_unix.c:548).
    NoRouteHost,
    /// Wire name keeps upstream's typo: `wait-tcp-respone-timeout`.
    WaitTcpResponseTimeout,
    UnexpectedError {
        errno: i64,
    },
    /// Always carries token 0.
    CommandParseError,
    /// Always carries token 0.
    CommandBufferOverflow,
}

impl ResponseKind {
    /// The six replies `handle_reply_errors()` (cmdpipe.c:690-728) treats as fatal.
    pub fn is_fatal_for_client(&self) -> bool {
        matches!(
            self,
            ResponseKind::ProbesExhausted
                | ResponseKind::InvalidArgument { .. }
                | ResponseKind::PermissionDenied
                | ResponseKind::AddressInUse
                | ResponseKind::AddressNotAvailable
                | ResponseKind::UnexpectedError { .. }
        )
    }

    fn name(&self) -> &'static str {
        match self {
            ResponseKind::Probe { result, .. } => result.as_str(),
            ResponseKind::NoReply => "no-reply",
            ResponseKind::FeatureSupport(_) => "feature-support",
            ResponseKind::InvalidArgument { .. } => "invalid-argument",
            ResponseKind::UnknownCommand => "unknown-command",
            ResponseKind::ProbesExhausted => "probes-exhausted",
            ResponseKind::PermissionDenied => "permission-denied",
            ResponseKind::AddressInUse => "address-in-use",
            ResponseKind::AddressNotAvailable => "address-not-available",
            ResponseKind::NetworkDown => "network-down",
            ResponseKind::HostDown => "host-down",
            ResponseKind::NoRouteNetwork => "no-route-network",
            ResponseKind::NoRouteHost => "no-route-host",
            ResponseKind::WaitTcpResponseTimeout => "wait-tcp-respone-timeout",
            ResponseKind::UnexpectedError { .. } => "unexpected-error",
            ResponseKind::CommandParseError => "command-parse-error",
            ResponseKind::CommandBufferOverflow => "command-buffer-overflow",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub token: i32,
    pub kind: ResponseKind,
}

fn bad(name: &'static str, value: &str) -> ParseError {
    ParseError::InvalidValue {
        name,
        value: value.to_string(),
    }
}

fn probe(l: &Line<'_>, result: ProbeResult) -> Result<ResponseKind, ParseError> {
    let addr: IpAddr = if let Some(v) = l.first("ip-4") {
        v.parse::<Ipv4Addr>().map_err(|_| bad("ip-4", v))?.into()
    } else if let Some(v) = l.first("ip-6") {
        v.parse::<Ipv6Addr>().map_err(|_| bad("ip-6", v))?.into()
    } else {
        return Err(ParseError::MissingArgument("ip-4"));
    };
    let rtt = l
        .first("round-trip-time")
        .ok_or(ParseError::MissingArgument("round-trip-time"))?;
    let rtt_us: u32 = rtt.parse().map_err(|_| bad("round-trip-time", rtt))?;
    // cmdpipe.c discards all labels when the list is malformed.
    let mpls = l
        .first("mpls")
        .map(|m| parse_mpls_list(m).unwrap_or_default())
        .unwrap_or_default();
    Ok(ResponseKind::Probe {
        result,
        addr,
        rtt_us,
        mpls,
    })
}

impl Response {
    /// Encode as one line including the trailing newline.
    pub fn encode(&self) -> String {
        let mut s = String::with_capacity(96);
        let _ = write!(s, "{} {}", self.token, self.kind.name());
        match &self.kind {
            ResponseKind::Probe {
                addr, rtt_us, mpls, ..
            } => {
                let key = if addr.is_ipv4() { "ip-4" } else { "ip-6" };
                let _ = write!(s, " {key} {addr} round-trip-time {rtt_us}");
                if !mpls.is_empty() {
                    s.push_str(" mpls ");
                    format_mpls_list(mpls, &mut s);
                }
            }
            ResponseKind::FeatureSupport(v) => {
                let _ = write!(s, " support {v}");
            }
            ResponseKind::InvalidArgument { reason: Some(r) } => {
                let _ = write!(s, " reason {}", r.as_str());
            }
            ResponseKind::UnexpectedError { errno } => {
                let _ = write!(s, " errno {errno}");
            }
            _ => {}
        }
        s.push('\n');
        s
    }

    /// Parse one line (trailing newline optional).
    pub fn parse(line: &str) -> Result<Response, ParseError> {
        let l = tokenize(line)?;
        let kind = match l.name {
            "reply" => probe(&l, ProbeResult::Reply)?,
            "ttl-expired" => probe(&l, ProbeResult::TtlExpired)?,
            "no-route-host" if l.first("ip-4").is_some() || l.first("ip-6").is_some() => {
                probe(&l, ProbeResult::NoRouteHost)?
            }
            "no-route-host" => ResponseKind::NoRouteHost,
            "no-reply" => ResponseKind::NoReply,
            "feature-support" => ResponseKind::FeatureSupport(
                l.first("support")
                    .ok_or(ParseError::MissingArgument("support"))?
                    .to_string(),
            ),
            "invalid-argument" => ResponseKind::InvalidArgument {
                reason: l.first("reason").and_then(InvalidReason::parse),
            },
            "unknown-command" => ResponseKind::UnknownCommand,
            "probes-exhausted" => ResponseKind::ProbesExhausted,
            "permission-denied" => ResponseKind::PermissionDenied,
            "address-in-use" => ResponseKind::AddressInUse,
            "address-not-available" => ResponseKind::AddressNotAvailable,
            "network-down" => ResponseKind::NetworkDown,
            "host-down" => ResponseKind::HostDown,
            "no-route-network" => ResponseKind::NoRouteNetwork,
            "wait-tcp-respone-timeout" => ResponseKind::WaitTcpResponseTimeout,
            "unexpected-error" => ResponseKind::UnexpectedError {
                errno: l.first("errno").and_then(|v| v.parse().ok()).unwrap_or(0),
            },
            "command-parse-error" => ResponseKind::CommandParseError,
            "command-buffer-overflow" => ResponseKind::CommandBufferOverflow,
            other => return Err(ParseError::UnknownCommand(other.to_string())),
        };
        Ok(Response {
            token: l.token,
            kind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MplsLabel;

    fn v4(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn encodes_reply_with_mpls() {
        let r = Response {
            token: 33000,
            kind: ResponseKind::Probe {
                result: ProbeResult::TtlExpired,
                addr: v4("10.0.0.1"),
                rtt_us: 1234,
                mpls: vec![
                    MplsLabel {
                        label: 16001,
                        tc: 0,
                        bottom_of_stack: true,
                        ttl: 1,
                    },
                    MplsLabel {
                        label: 16002,
                        tc: 2,
                        bottom_of_stack: false,
                        ttl: 255,
                    },
                ],
            },
        };
        assert_eq!(
            r.encode(),
            "33000 ttl-expired ip-4 10.0.0.1 round-trip-time 1234 mpls 16001,0,1,1,16002,2,0,255\n"
        );
    }

    #[test]
    fn parses_reply() {
        let r = Response::parse("33001 reply ip-4 192.0.2.1 round-trip-time 987\n").unwrap();
        assert_eq!(
            r,
            Response {
                token: 33001,
                kind: ResponseKind::Probe {
                    result: ProbeResult::Reply,
                    addr: v4("192.0.2.1"),
                    rtt_us: 987,
                    mpls: vec![]
                },
            }
        );
        let r = Response::parse("2 reply ip-6 2001:db8::1 round-trip-time 5").unwrap();
        assert!(matches!(
            r.kind,
            ResponseKind::Probe {
                addr: IpAddr::V6(_),
                ..
            }
        ));
    }

    #[test]
    fn no_route_host_has_two_forms() {
        assert_eq!(
            Response::parse("7 no-route-host").unwrap().kind,
            ResponseKind::NoRouteHost
        );
        assert!(matches!(
            Response::parse("7 no-route-host ip-4 10.0.0.9 round-trip-time 5")
                .unwrap()
                .kind,
            ResponseKind::Probe {
                result: ProbeResult::NoRouteHost,
                ..
            }
        ));
    }

    #[test]
    fn feature_support_carries_ok_no_or_a_version() {
        assert_eq!(
            Response::parse("1 feature-support support ok")
                .unwrap()
                .kind,
            ResponseKind::FeatureSupport("ok".into())
        );
        assert_eq!(
            Response::parse("1 feature-support support 0.96")
                .unwrap()
                .kind,
            ResponseKind::FeatureSupport("0.96".into())
        );
        assert_eq!(
            Response {
                token: 1,
                kind: ResponseKind::FeatureSupport("no".into())
            }
            .encode(),
            "1 feature-support support no\n"
        );
    }

    #[test]
    fn keeps_upstream_typo_on_the_wire() {
        let r = Response {
            token: 3,
            kind: ResponseKind::WaitTcpResponseTimeout,
        };
        assert_eq!(r.encode(), "3 wait-tcp-respone-timeout\n");
        assert_eq!(Response::parse(&r.encode()).unwrap(), r);
    }

    #[test]
    fn parses_and_encodes_every_bare_response() {
        let cases = [
            ("0 command-parse-error", ResponseKind::CommandParseError),
            (
                "0 command-buffer-overflow",
                ResponseKind::CommandBufferOverflow,
            ),
            ("5 unknown-command", ResponseKind::UnknownCommand),
            ("5 probes-exhausted", ResponseKind::ProbesExhausted),
            ("5 permission-denied", ResponseKind::PermissionDenied),
            ("5 address-in-use", ResponseKind::AddressInUse),
            ("5 address-not-available", ResponseKind::AddressNotAvailable),
            ("5 network-down", ResponseKind::NetworkDown),
            ("5 host-down", ResponseKind::HostDown),
            ("5 no-route-network", ResponseKind::NoRouteNetwork),
            ("5 no-reply", ResponseKind::NoReply),
            (
                "5 unexpected-error errno 22",
                ResponseKind::UnexpectedError { errno: 22 },
            ),
            (
                "5 invalid-argument",
                ResponseKind::InvalidArgument { reason: None },
            ),
            (
                "5 invalid-argument reason protocol-not-supported",
                ResponseKind::InvalidArgument {
                    reason: Some(InvalidReason::ProtocolNotSupported),
                },
            ),
            (
                "5 invalid-argument reason ip-version-not-supported",
                ResponseKind::InvalidArgument {
                    reason: Some(InvalidReason::IpVersionNotSupported),
                },
            ),
        ];
        for (line, kind) in cases {
            let r = Response::parse(line).unwrap();
            assert_eq!(r.kind, kind, "{line}");
            assert_eq!(r.encode().trim_end(), line, "{line}");
        }
    }

    #[test]
    fn malformed_mpls_means_no_labels_like_cmdpipe() {
        let r = Response::parse("1 reply ip-4 10.0.0.1 round-trip-time 5 mpls 1,x").unwrap();
        assert!(matches!(r.kind, ResponseKind::Probe { ref mpls, .. } if mpls.is_empty()));
    }

    #[test]
    fn probe_replies_need_address_and_rtt() {
        assert!(Response::parse("1 reply round-trip-time 5").is_err());
        assert!(Response::parse("1 reply ip-4 10.0.0.1").is_err());
        assert!(Response::parse("1 reply ip-4 10.0.0.1 round-trip-time -5").is_err());
        assert!(Response::parse("1 frobnicate").is_err());
    }

    #[test]
    fn fatal_set_matches_cmdpipe_handle_reply_errors() {
        for k in [
            ResponseKind::ProbesExhausted,
            ResponseKind::InvalidArgument { reason: None },
            ResponseKind::PermissionDenied,
            ResponseKind::AddressInUse,
            ResponseKind::AddressNotAvailable,
            ResponseKind::UnexpectedError { errno: 1 },
        ] {
            assert!(k.is_fatal_for_client(), "{k:?}");
        }
        for k in [
            ResponseKind::NoReply,
            ResponseKind::NetworkDown,
            ResponseKind::NoRouteHost,
            ResponseKind::CommandParseError,
        ] {
            assert!(!k.is_fatal_for_client(), "{k:?}");
        }
    }
}
