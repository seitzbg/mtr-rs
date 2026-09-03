//! Platform boundary of the helper. The trait mirrors what packet/probe.h expects of
//! probe_unix.c; `error_response` ports `report_packet_error()` (packet/probe_unix.c:532-556,
//! mtr 0.96, commit 7b01773). GPL-2.0-only.

use std::os::fd::BorrowedFd;
use std::time::Instant;

use mtr_proto::{CProbeParams, Protocol, Response, ResponseKind};

use crate::probe_table::ProbeTable;

#[cfg(test)]
pub mod fake;
// `linux` arrives in Task 7 (LinuxBackend); until then this line would fail to build natively
// on Linux since the module doesn't exist yet.
#[cfg(target_os = "macos")]
pub mod macos;

pub trait ProbeBackend {
    fn ip_version_supported(&self, version: u8) -> bool;
    fn protocol_supported(&self, protocol: Protocol) -> bool;
    fn mark_supported(&self) -> bool;
    fn send_probe(
        &mut self,
        table: &mut ProbeTable,
        idx: usize,
        params: &CProbeParams,
    ) -> std::io::Result<()>;
    fn recv_fds(&self) -> Vec<BorrowedFd<'_>>;
    fn receive(&mut self, table: &mut ProbeTable, now: Instant, out: &mut Vec<Response>);
}

/// `report_packet_error()`: errno → reply name.
pub fn error_response(token: i32, err: &std::io::Error) -> Response {
    use nix::libc as c;
    let kind = match err.raw_os_error() {
        Some(c::EINVAL) => ResponseKind::InvalidArgument { reason: None },
        Some(c::ENETDOWN) => ResponseKind::NetworkDown,
        Some(c::EHOSTDOWN) => ResponseKind::HostDown,
        Some(c::ENETUNREACH) => ResponseKind::NoRouteNetwork,
        Some(c::EHOSTUNREACH) => ResponseKind::NoRouteHost,
        Some(c::EPERM) => ResponseKind::PermissionDenied,
        Some(c::EADDRINUSE) => ResponseKind::AddressInUse,
        Some(c::EADDRNOTAVAIL) => ResponseKind::AddressNotAvailable,
        Some(c::ETIMEDOUT) => ResponseKind::WaitTcpResponseTimeout,
        other => ResponseKind::UnexpectedError {
            errno: other.map(i64::from),
        },
    };
    Response { token, kind }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtr_proto::ResponseKind;
    use std::io::Error;

    #[test]
    fn errno_maps_to_the_c_reply_names() {
        let cases = [
            (
                nix::libc::EINVAL,
                ResponseKind::InvalidArgument { reason: None },
            ),
            (nix::libc::ENETDOWN, ResponseKind::NetworkDown),
            (nix::libc::EHOSTDOWN, ResponseKind::HostDown),
            (nix::libc::ENETUNREACH, ResponseKind::NoRouteNetwork),
            (nix::libc::EHOSTUNREACH, ResponseKind::NoRouteHost),
            (nix::libc::EPERM, ResponseKind::PermissionDenied),
            (nix::libc::EADDRINUSE, ResponseKind::AddressInUse),
            (nix::libc::EADDRNOTAVAIL, ResponseKind::AddressNotAvailable),
            (nix::libc::ETIMEDOUT, ResponseKind::WaitTcpResponseTimeout),
            (
                nix::libc::ENOBUFS,
                ResponseKind::UnexpectedError {
                    errno: Some(i64::from(nix::libc::ENOBUFS)),
                },
            ),
        ];
        for (errno, kind) in cases {
            let r = error_response(7, &Error::from_raw_os_error(errno));
            assert_eq!(r.token, 7);
            assert_eq!(r.kind, kind, "errno {errno}");
        }
        // An error without an OS code is "unexpected" with no errno.
        let r = error_response(7, &Error::other("boom"));
        assert_eq!(r.kind, ResponseKind::UnexpectedError { errno: None });
    }
}
