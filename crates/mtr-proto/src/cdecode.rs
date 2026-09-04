//! Helper-side decoding with C semantics: `decode_probe_argument()` / `send_probe_command()`
//! and `find_parameter()`. Ported from packet/command.c:44-63, 163-354 (mtr 0.96, commit
//! 7b01773). GPL-2.0-only.
//!
//! `Request::parse` is the client's strict view of the wire; this module is what the helper
//! must accept: `strtol` numbers narrowed to C `int`, unknown argument names ignored, and the
//! first bad occurrence rejected.

use crate::request::Protocol;
use crate::tokenize::Line;

/// The helper answers `<token> invalid-argument`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidArgument;

/// `probe_param_t` (probe.h:43-88) with `send_probe_command()`'s defaults (command.c:332-336).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CProbeParams {
    pub ip_version: u8,
    pub remote_address: Option<String>,
    pub local_address: Option<String>,
    pub local_device: Option<String>,
    pub protocol: Protocol,
    pub dest_port: i32,
    pub local_port: i32,
    pub type_of_service: i32,
    pub routing_mark: u32,
    pub ttl: i32,
    pub packet_size: i32,
    pub bit_pattern: i32,
    pub timeout: i32,
}

impl Default for CProbeParams {
    fn default() -> Self {
        CProbeParams {
            ip_version: 0,
            remote_address: None,
            local_address: None,
            local_device: None,
            protocol: Protocol::Icmp,
            dest_port: 0,
            local_port: 0,
            type_of_service: 0,
            routing_mark: 0,
            ttl: 255,
            packet_size: 64,
            bit_pattern: 0,
            timeout: 10,
        }
    }
}

fn is_c_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r')
}

/// `strtol(value, &end, 10)` followed by C's `*end != 0` check (command.c:216-218 and the
/// seven identical blocks after it). Leading whitespace is skipped as strtol does; the rest
/// must be `[+-]?[0-9]+`. Out-of-range values saturate to `LONG_MAX`/`LONG_MIN` (command.c
/// ignores errno here).
pub fn strtol_full(s: &str) -> Option<i64> {
    let t = s.trim_start_matches(is_c_space);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut acc: i64 = 0;
    for b in digits.bytes() {
        let d = i64::from(b - b'0');
        acc = if neg {
            acc.checked_mul(10)
                .and_then(|v| v.checked_sub(d))
                .unwrap_or(i64::MIN)
        } else {
            acc.checked_mul(10)
                .and_then(|v| v.checked_add(d))
                .unwrap_or(i64::MAX)
        };
    }
    Some(acc)
}

/// `long` assigned to a C `int`: two's-complement wrap.
fn c_int(v: &str) -> Result<i32, InvalidArgument> {
    strtol_full(v).map(|n| n as i32).ok_or(InvalidArgument)
}

/// `find_parameter(command, "feature")` (command.c:44-63, 148).
pub fn check_support_feature<'a>(line: &Line<'a>) -> Option<&'a str> {
    line.first("feature")
}

/// `send_probe_command()` + `decode_probe_argument()` (command.c:163-354): defaults, then every
/// argument in order; the first one that fails ends the command with `invalid-argument`.
pub fn decode_send_probe(line: &Line<'_>) -> Result<CProbeParams, InvalidArgument> {
    let mut p = CProbeParams::default();
    for (name, value) in &line.args {
        match *name {
            "ip-4" => {
                p.ip_version = 4;
                p.remote_address = Some((*value).to_string());
            }
            "ip-6" => {
                p.ip_version = 6;
                p.remote_address = Some((*value).to_string());
            }
            "local-ip-4" | "local-ip-6" => p.local_address = Some((*value).to_string()),
            "local-device" => p.local_device = Some((*value).to_string()),
            "protocol" => p.protocol = Protocol::parse(value).ok_or(InvalidArgument)?,
            "port" => p.dest_port = c_int(value)?,
            "local-port" => {
                let port = c_int(value)?;
                if port < 1024 {
                    return Err(InvalidArgument);
                }
                p.local_port = port;
            }
            "tos" => p.type_of_service = c_int(value)?,
            "mark" => p.routing_mark = strtol_full(value).ok_or(InvalidArgument)? as u32,
            "size" => p.packet_size = c_int(value)?,
            "bit-pattern" => p.bit_pattern = c_int(value)?,
            "ttl" => p.ttl = c_int(value)?,
            "timeout" => p.timeout = c_int(value)?,
            _ => {}
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenize::tokenize;

    fn decode(s: &str) -> Result<CProbeParams, InvalidArgument> {
        decode_send_probe(&tokenize(s).unwrap())
    }

    #[test]
    fn strtol_full_accepts_whole_decimal_integers_only() {
        assert_eq!(strtol_full("42"), Some(42));
        assert_eq!(strtol_full("-7"), Some(-7));
        assert_eq!(strtol_full("+9"), Some(9));
        assert_eq!(strtol_full("  12"), Some(12)); // strtol skips leading whitespace
        assert_eq!(strtol_full("12abc"), None); // *endstr != 0
        assert_eq!(strtol_full("abc"), None);
        assert_eq!(strtol_full(""), None);
        assert_eq!(strtol_full("-"), None);
        assert_eq!(strtol_full("99999999999999999999"), Some(i64::MAX)); // LONG_MAX
        assert_eq!(strtol_full("-99999999999999999999"), Some(i64::MIN));
    }

    #[test]
    fn defaults_match_send_probe_command() {
        let p = decode("1 send-probe ip-4 8.8.8.8").unwrap();
        assert_eq!(p.ip_version, 4);
        assert_eq!(p.remote_address.as_deref(), Some("8.8.8.8"));
        assert_eq!(
            (p.protocol, p.ttl, p.packet_size, p.timeout),
            (Protocol::Icmp, 255, 64, 10)
        );
        assert_eq!(
            (p.dest_port, p.local_port, p.type_of_service, p.routing_mark),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn missing_address_leaves_ip_version_zero_and_string_addresses_pass_through() {
        // command.c does not validate the address text; probe.c's inet_pton does later.
        assert_eq!(decode("22 send-probe").unwrap().ip_version, 0);
        let p = decode("23 send-probe ip-4 str-value").unwrap();
        assert_eq!(p.remote_address.as_deref(), Some("str-value"));
        let p = decode("2 send-probe ip-6 ::1 local-ip-6 ::1 local-device lo").unwrap();
        assert_eq!(
            (
                p.ip_version,
                p.local_address.as_deref(),
                p.local_device.as_deref()
            ),
            (6, Some("::1"), Some("lo"))
        );
    }

    #[test]
    fn numeric_arguments_use_strtol_and_reject_trailing_garbage() {
        assert_eq!(
            decode("24 send-probe ip-4 8.8.8.8 timeout str-value"),
            Err(InvalidArgument)
        );
        assert_eq!(
            decode("25 send-probe ip-4 8.8.8.8 ttl str-value"),
            Err(InvalidArgument)
        );
        let p = decode(
            "3 send-probe ip-4 8.8.8.8 ttl 300 size -5 tos 62 mark 7 bit-pattern 44 timeout 0 port 164",
        )
        .unwrap();
        assert_eq!(
            (p.ttl, p.packet_size, p.type_of_service, p.routing_mark),
            (300, -5, 62, 7)
        );
        assert_eq!((p.bit_pattern, p.timeout, p.dest_port), (44, 0, 164));
        // long → int narrowing wraps, as the C assignment does
        assert_eq!(
            decode("4 send-probe ip-4 8.8.8.8 ttl 4294967297")
                .unwrap()
                .ttl,
            1
        );
    }

    #[test]
    fn local_port_below_1024_is_rejected_and_unknown_names_are_ignored() {
        assert_eq!(
            decode("5 send-probe ip-4 8.8.8.8 local-port 80"),
            Err(InvalidArgument)
        );
        assert_eq!(
            decode("5 send-probe ip-4 8.8.8.8 local-port 1991")
                .unwrap()
                .local_port,
            1991
        );
        assert_eq!(
            decode("6 send-probe ip-4 8.8.8.8 frobnicate 9")
                .unwrap()
                .ip_version,
            4
        );
    }

    #[test]
    fn protocol_names_and_first_bad_occurrence() {
        assert_eq!(
            decode("7 send-probe protocol udp ip-4 1.1.1.1")
                .unwrap()
                .protocol,
            Protocol::Udp
        );
        assert_eq!(
            decode("7 send-probe protocol sctp ip-4 1.1.1.1")
                .unwrap()
                .protocol,
            Protocol::Sctp
        );
        assert_eq!(
            decode("7 send-probe protocol gre ip-4 1.1.1.1"),
            Err(InvalidArgument)
        );
        // later occurrences win, but a bad earlier one already failed the command
        assert_eq!(
            decode("8 send-probe ip-4 1.1.1.1 ttl 3 ttl 9").unwrap().ttl,
            9
        );
        assert_eq!(
            decode("8 send-probe ip-4 1.1.1.1 ttl x ttl 9"),
            Err(InvalidArgument)
        );
    }

    #[test]
    fn check_support_feature_is_the_first_occurrence() {
        let l = tokenize("30 check-support feature version feature ip-4").unwrap();
        assert_eq!(check_support_feature(&l), Some("version"));
        assert_eq!(
            check_support_feature(&tokenize("31 check-support").unwrap()),
            None
        );
    }
}
