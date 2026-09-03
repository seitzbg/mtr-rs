//! macOS stub: proves the trait boundary, probes nothing (spec §6). GPL-2.0-only.
use super::ProbeBackend;
use crate::probe_table::ProbeTable;
use mtr_proto::{CProbeParams, Protocol, Response};
use std::os::fd::BorrowedFd;
use std::time::Instant;

pub struct MacosBackend;

impl MacosBackend {
    pub fn open_privileged() -> Result<Self, crate::Fatal> {
        Ok(MacosBackend)
    }
    pub fn finish_init(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl ProbeBackend for MacosBackend {
    fn ip_version_supported(&self, _: u8) -> bool {
        false
    }
    fn protocol_supported(&self, _: Protocol) -> bool {
        false
    }
    fn mark_supported(&self) -> bool {
        false
    }
    fn send_probe(
        &mut self,
        _: &mut ProbeTable,
        _: usize,
        _: &CProbeParams,
    ) -> std::io::Result<()> {
        Err(std::io::Error::from_raw_os_error(nix::libc::EPERM))
    }
    fn recv_fds(&self) -> Vec<BorrowedFd<'_>> {
        Vec::new()
    }
    fn receive(&mut self, _: &mut ProbeTable, _: Instant, _: &mut Vec<Response>) {}
}
