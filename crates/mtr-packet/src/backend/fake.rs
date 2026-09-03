//! In-memory backend for unit tests of the command layer and the serve loop. GPL-2.0-only.
use super::ProbeBackend;
use crate::probe_table::ProbeTable;
use mtr_proto::{CProbeParams, ProbeResult, Protocol, Response, ResponseKind};
use std::net::IpAddr;
use std::os::fd::BorrowedFd;
use std::time::Instant;

#[derive(Default)]
pub struct FakeBackend {
    pub v4: bool,
    pub v6: bool,
    pub sctp: bool,
    pub sent: Vec<(i32, CProbeParams)>,
    pub fail_with: Option<i32>,
    pub reply_immediately: bool,
}

impl FakeBackend {
    pub fn v4_only() -> Self {
        FakeBackend {
            v4: true,
            ..Default::default()
        }
    }
}

impl ProbeBackend for FakeBackend {
    fn ip_version_supported(&self, version: u8) -> bool {
        (version == 4 && self.v4) || (version == 6 && self.v6)
    }
    fn protocol_supported(&self, protocol: Protocol) -> bool {
        protocol != Protocol::Sctp || self.sctp
    }
    fn mark_supported(&self) -> bool {
        true
    }
    fn send_probe(
        &mut self,
        table: &mut ProbeTable,
        idx: usize,
        params: &CProbeParams,
    ) -> std::io::Result<()> {
        if let Some(errno) = self.fail_with {
            return Err(std::io::Error::from_raw_os_error(errno));
        }
        let remote: IpAddr = params
            .remote_address
            .as_deref()
            .unwrap_or("")
            .parse()
            .map_err(|_| std::io::Error::from_raw_os_error(nix::libc::EINVAL))?;
        table.probes[idx].remote = std::net::SocketAddr::new(remote, 0);
        self.sent.push((table.probes[idx].token, params.clone()));
        Ok(())
    }
    fn recv_fds(&self) -> Vec<BorrowedFd<'_>> {
        Vec::new()
    }
    fn receive(&mut self, table: &mut ProbeTable, _now: Instant, out: &mut Vec<Response>) {
        if !self.reply_immediately {
            return;
        }
        while let Some(p) = table.probes.pop() {
            out.push(Response {
                token: p.token,
                kind: ResponseKind::Probe {
                    result: ProbeResult::Reply,
                    addr: p.remote.ip(),
                    rtt_us: 1000,
                    mpls: Vec::new(),
                },
            });
        }
    }
}
