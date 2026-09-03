//! Command stream buffering and dispatch. Ported from packet/command.c (mtr 0.96, commit
//! 7b01773). GPL-2.0-only.

use std::time::Instant;

use mtr_proto::tokenize::{Line, tokenize};
use mtr_proto::{
    COMMAND_BUFFER_SIZE, InvalidReason, ProbeResult, Protocol, Response, ResponseKind,
    check_support_feature, decode_send_probe,
};

use crate::backend::{ProbeBackend, error_response};
use crate::probe_table::{ProbeTable, rtt_us};

/// `command_buffer_t` (command.h:27-36): a 4096-byte window over stdin.
#[derive(Default)]
pub struct CommandBuffer {
    buf: Vec<u8>,
}

impl CommandBuffer {
    pub fn new() -> Self {
        CommandBuffer {
            buf: Vec::with_capacity(COMMAND_BUFFER_SIZE),
        }
    }

    /// `read_commands()` reads at most this many bytes (command.c:469-470).
    pub fn space_remaining(&self) -> usize {
        COMMAND_BUFFER_SIZE - 1 - self.buf.len()
    }

    pub fn push(&mut self, data: &[u8]) {
        debug_assert!(data.len() <= self.space_remaining());
        self.buf.extend_from_slice(data);
    }

    /// `dispatch_buffer_commands()` (command.c:379-438): every `\n`-terminated line, then the
    /// overflow check — a full buffer without a newline is discarded and reported once.
    pub fn take_lines(&mut self) -> (Vec<String>, bool) {
        let mut lines = Vec::new();
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=nl).collect();
            lines.push(String::from_utf8_lossy(&line[..nl]).into_owned());
        }
        let overflow = self.buf.len() >= COMMAND_BUFFER_SIZE - 1;
        if overflow {
            self.buf.clear();
        }
        (lines, overflow)
    }
}

/// The dispatcher: `net_state_t` + `dispatch_command()`.
pub struct Helper<B: ProbeBackend> {
    pub backend: B,
    pub table: ProbeTable,
}

impl<B: ProbeBackend> Helper<B> {
    pub fn new(backend: B) -> Self {
        Helper {
            backend,
            table: ProbeTable::new(),
        }
    }

    /// `check_support()` (command.c:93-137).
    pub fn support_string(&self, feature: &str) -> String {
        let ok = |b: bool| if b { "ok" } else { "no" }.to_string();
        match feature {
            "version" => crate::VERSION.to_string(),
            "ip-4" => ok(self.backend.ip_version_supported(4)),
            "ip-6" => ok(self.backend.ip_version_supported(6)),
            "send-probe" => ok(true),
            "icmp" => ok(self.backend.protocol_supported(Protocol::Icmp)),
            "udp" => ok(self.backend.protocol_supported(Protocol::Udp)),
            "tcp" => ok(self.backend.protocol_supported(Protocol::Tcp)),
            "sctp" => ok(self.backend.protocol_supported(Protocol::Sctp)),
            "mark" => ok(self.backend.mark_supported()),
            _ => ok(false),
        }
    }

    /// `parse_command()` + `dispatch_command()` for one complete line.
    pub fn dispatch_line(&mut self, line: &str, now: Instant, out: &mut Vec<Response>) {
        let l = match tokenize(line) {
            Ok(l) => l,
            Err(_) => {
                out.push(Response {
                    token: 0,
                    kind: ResponseKind::CommandParseError,
                });
                return;
            }
        };
        match l.name {
            "check-support" => {
                let kind = match check_support_feature(&l) {
                    None => ResponseKind::InvalidArgument { reason: None },
                    Some(f) => ResponseKind::FeatureSupport(self.support_string(f)),
                };
                out.push(Response {
                    token: l.token,
                    kind,
                });
            }
            "send-probe" => self.send_probe(&l, now, out),
            _ => out.push(Response {
                token: l.token,
                kind: ResponseKind::UnknownCommand,
            }),
        }
    }

    /// `send_probe_command()` (command.c:320-354) + `send_probe()` error handling
    /// (probe_unix.c:559-636).
    fn send_probe(&mut self, l: &Line<'_>, now: Instant, out: &mut Vec<Response>) {
        let token = l.token;
        let Ok(params) = decode_send_probe(l) else {
            out.push(Response {
                token,
                kind: ResponseKind::InvalidArgument { reason: None },
            });
            return;
        };
        if !self.backend.ip_version_supported(params.ip_version) {
            out.push(Response {
                token,
                kind: ResponseKind::InvalidArgument {
                    reason: Some(InvalidReason::IpVersionNotSupported),
                },
            });
            return;
        }
        if !self.backend.protocol_supported(params.protocol) {
            out.push(Response {
                token,
                kind: ResponseKind::InvalidArgument {
                    reason: Some(InvalidReason::ProtocolNotSupported),
                },
            });
            return;
        }
        let Some(idx) = self.table.alloc(token, now, params.timeout) else {
            out.push(Response {
                token,
                kind: ResponseKind::ProbesExhausted,
            });
            return;
        };
        self.table.probes[idx].protocol = params.protocol;
        match self.backend.send_probe(&mut self.table, idx, &params) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(nix::libc::ECONNREFUSED) => {
                // A refused stream connect means the destination was reached (probe_unix.c:617-619).
                let p = self.table.remove(idx);
                out.push(Response {
                    token,
                    kind: ResponseKind::Probe {
                        result: ProbeResult::Reply,
                        addr: p.remote.ip(),
                        rtt_us: rtt_us(p.departure, now),
                        mpls: Vec::new(),
                    },
                });
            }
            Err(e) => {
                self.table.remove(idx);
                out.push(error_response(token, &e));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::fake::FakeBackend;
    use mtr_proto::{InvalidReason, ProbeResult, ResponseKind};

    fn helper() -> Helper<FakeBackend> {
        Helper::new(FakeBackend::v4_only())
    }

    fn one(h: &mut Helper<FakeBackend>, line: &str) -> Response {
        let mut out = Vec::new();
        h.dispatch_line(line, Instant::now(), &mut out);
        assert_eq!(out.len(), 1, "{line}: {out:?}");
        out.pop().unwrap()
    }

    #[test]
    fn buffer_pops_complete_lines_and_keeps_the_partial_tail() {
        let mut b = CommandBuffer::new();
        assert_eq!(b.space_remaining(), COMMAND_BUFFER_SIZE - 1);
        b.push(b"1 a\n2 b\n3 par");
        let (lines, overflow) = b.take_lines();
        assert_eq!(lines, vec!["1 a".to_string(), "2 b".to_string()]);
        assert!(!overflow);
        b.push(b"tial\n");
        assert_eq!(b.take_lines().0, vec!["3 partial".to_string()]);
    }

    #[test]
    fn buffer_full_without_newline_reports_overflow_once_and_resets() {
        let mut b = CommandBuffer::new();
        let chunk = vec![b'x'; b.space_remaining()];
        b.push(&chunk);
        assert_eq!(b.space_remaining(), 0);
        let (lines, overflow) = b.take_lines();
        assert!(lines.is_empty());
        assert!(overflow);
        assert_eq!(b.space_remaining(), COMMAND_BUFFER_SIZE - 1);
        assert!(!b.take_lines().1);
    }

    #[test]
    fn unknown_and_malformed_commands() {
        let mut h = helper();
        assert_eq!(
            one(&mut h, "13 argle-bargle").encode(),
            "13 unknown-command\n"
        );
        assert_eq!(one(&mut h, "malformed").encode(), "0 command-parse-error\n");
        assert_eq!(
            one(&mut h, "5 send-probe ttl").encode(),
            "0 command-parse-error\n"
        );
    }

    #[test]
    fn versioning_and_feature_support() {
        let mut h = helper();
        assert_eq!(
            one(&mut h, "30 check-support feature version").encode(),
            format!("30 feature-support support {}\n", crate::VERSION)
        );
        assert_eq!(
            one(&mut h, "31 check-support feature ip-4").encode(),
            "31 feature-support support ok\n"
        );
        assert_eq!(
            one(&mut h, "31 check-support feature ip-6").encode(),
            "31 feature-support support no\n"
        );
        assert_eq!(
            one(&mut h, "32 check-support feature send-probe").encode(),
            "32 feature-support support ok\n"
        );
        assert_eq!(
            one(&mut h, "33 check-support feature bogus-feature").encode(),
            "33 feature-support support no\n"
        );
        assert_eq!(
            one(&mut h, "34 check-support feature sctp").encode(),
            "34 feature-support support no\n"
        );
        assert_eq!(
            one(&mut h, "35 check-support feature mark").encode(),
            "35 feature-support support ok\n"
        );
        assert_eq!(
            one(&mut h, "36 check-support").kind,
            ResponseKind::InvalidArgument { reason: None }
        );
    }

    #[test]
    fn invalid_arguments_like_cmdparse_py() {
        let mut h = helper();
        assert_eq!(
            one(&mut h, "22 send-probe").kind,
            ResponseKind::InvalidArgument {
                reason: Some(InvalidReason::IpVersionNotSupported)
            }
        );
        assert_eq!(
            one(&mut h, "23 send-probe ip-4 str-value").kind,
            ResponseKind::InvalidArgument { reason: None }
        );
        assert_eq!(
            one(&mut h, "24 send-probe ip-4 8.8.8.8 timeout str-value").kind,
            ResponseKind::InvalidArgument { reason: None }
        );
        assert_eq!(
            one(&mut h, "25 send-probe ip-4 8.8.8.8 ttl str-value").kind,
            ResponseKind::InvalidArgument { reason: None }
        );
        assert_eq!(
            one(&mut h, "26 send-probe ip-6 ::1").kind,
            ResponseKind::InvalidArgument {
                reason: Some(InvalidReason::IpVersionNotSupported)
            }
        );
        assert_eq!(
            one(&mut h, "27 send-probe ip-4 8.8.8.8 protocol sctp").kind,
            ResponseKind::InvalidArgument {
                reason: Some(InvalidReason::ProtocolNotSupported)
            }
        );
        assert!(h.table.is_empty(), "failed probes are freed");
    }

    #[test]
    fn a_good_probe_is_handed_to_the_backend_and_stays_outstanding() {
        let mut h = helper();
        let mut out = Vec::new();
        h.dispatch_line(
            "15 send-probe ip-4 8.8.254.254 timeout 1",
            Instant::now(),
            &mut out,
        );
        assert!(out.is_empty());
        assert_eq!(h.table.len(), 1);
        assert_eq!(h.backend.sent[0].0, 15);
        assert_eq!(h.backend.sent[0].1.timeout, 1);
    }

    #[test]
    fn send_errors_map_through_report_packet_error_and_free_the_probe() {
        let mut h = helper();
        h.backend.fail_with = Some(nix::libc::EPERM);
        assert_eq!(
            one(&mut h, "9 send-probe ip-4 8.8.8.8").kind,
            ResponseKind::PermissionDenied
        );
        assert!(h.table.is_empty());
        // ECONNREFUSED from a stream connect means the destination answered: an immediate reply.
        h.backend.fail_with = Some(nix::libc::ECONNREFUSED);
        let r = one(&mut h, "10 send-probe ip-4 127.0.0.1 protocol tcp port 164");
        match r.kind {
            ResponseKind::Probe {
                result: ProbeResult::Reply,
                addr,
                ..
            } => assert_eq!(addr, "127.0.0.1".parse::<std::net::IpAddr>().unwrap()),
            other => panic!("{other:?}"),
        }
        assert!(h.table.is_empty());
    }

    #[test]
    fn probes_exhausted_when_the_table_is_full() {
        let mut h = helper();
        let now = Instant::now();
        for i in 0..crate::probe_table::MAX_PROBES {
            h.table.alloc(i as i32, now, 60).unwrap();
        }
        assert_eq!(
            one(&mut h, "99 send-probe ip-4 8.8.8.8").kind,
            ResponseKind::ProbesExhausted
        );
    }
}
