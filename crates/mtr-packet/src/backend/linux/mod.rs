//! Linux backend: raw sockets with the unprivileged DGRAM fallback. Ported from
//! packet/probe_unix.c (mtr 0.96, commit 7b01773). GPL-2.0-only.

pub mod construct;
pub mod deconstruct;
pub mod sockets;

use std::os::fd::BorrowedFd;
use std::time::Instant;

use mtr_proto::{CProbeParams, Protocol, Response};

use super::ProbeBackend;
use crate::Fatal;
use crate::probe_table::ProbeTable;
use sockets::Family;

pub struct LinuxBackend {
    pub v4: Option<Family>,
    pub v6: Option<Family>,
    pub sctp: bool,
    /// `htons(getpid())` as the ICMP id, see probe.c:193 and construct_unix.c:118.
    pub icmp_id: u16,
}

impl LinuxBackend {
    /// `init_net_state_privileged()` (probe_unix.c:422-459).
    pub fn open_privileged() -> Result<Self, Fatal> {
        let v4 = Family::open(4);
        let v6 = Family::open(6);
        if v4.is_err() && v6.is_err() {
            let e4 = v4.err().map(|e| e.to_string()).unwrap_or_default();
            let e6 = v6.err().map(|e| e.to_string()).unwrap_or_default();
            return Err(Fatal::Message(format!(
                "Failure to open IPv4 sockets: {e4}\nmtr-packet: Failure to open IPv6 sockets: {e6}"
            )));
        }
        Ok(LinuxBackend {
            v4: v4.ok(),
            v6: v6.ok(),
            sctp: false,
            icmp_id: (nix::unistd::getpid().as_raw() as u32 & 0xffff) as u16,
        })
    }

    /// `init_net_state()` (probe_unix.c:465-486), run after the privilege drop.
    pub fn finish_init(&mut self) -> std::io::Result<()> {
        for f in [&self.v4, &self.v6].into_iter().flatten() {
            f.set_nonblocking()?;
        }
        self.sctp = sockets::check_sctp_support();
        Ok(())
    }

    pub fn family(&self, version: u8) -> Option<&Family> {
        match version {
            4 => self.v4.as_ref(),
            6 => self.v6.as_ref(),
            _ => None,
        }
    }
}

impl ProbeBackend for LinuxBackend {
    fn ip_version_supported(&self, version: u8) -> bool {
        self.family(version).is_some()
    }
    fn protocol_supported(&self, protocol: Protocol) -> bool {
        protocol != Protocol::Sctp || self.sctp
    }
    fn mark_supported(&self) -> bool {
        true
    }
    fn send_probe(
        &mut self,
        _table: &mut ProbeTable,
        _idx: usize,
        _params: &CProbeParams,
    ) -> std::io::Result<()> {
        Err(std::io::Error::from_raw_os_error(nix::libc::EINVAL)) // Task 10
    }
    fn recv_fds(&self) -> Vec<BorrowedFd<'_>> {
        [&self.v4, &self.v6]
            .into_iter()
            .flatten()
            .flat_map(Family::recv_fds)
            .collect()
    }
    fn receive(&mut self, _table: &mut ProbeTable, _now: Instant, _out: &mut Vec<Response>) {}
}
