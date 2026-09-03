//! mtr-packet: the privileged probe helper of mtr-rs. Rust port of mtr 0.96's `packet/`
//! (commit 7b01773). GPL-2.0-only.
#![forbid(unsafe_code)]

pub mod backend;
pub mod command;
pub mod privs;
pub mod probe_table;

use std::io::Write;
use std::os::fd::BorrowedFd;
use std::time::Instant;

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

use crate::backend::ProbeBackend;
use crate::command::{CommandBuffer, Helper};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, thiserror::Error)]
pub enum Fatal {
    #[error("{0}")]
    Message(String),
    #[error("{0}: {1}")]
    Io(String, std::io::Error),
}

pub fn run() -> Result<(), Fatal> {
    Err(Fatal::Message("not implemented".into()))
}

/// `init_command_buffer()` / `set_socket_nonblocking()`: add `O_NONBLOCK`.
pub fn set_nonblocking(fd: BorrowedFd<'_>) -> nix::Result<()> {
    use nix::fcntl::{FcntlArg, OFlag, fcntl};
    let flags = OFlag::from_bits_truncate(fcntl(fd, FcntlArg::F_GETFL)?);
    fcntl(fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).map(|_| ())
}

/// The main loop of packet.c:131-167: flush, wait, receive replies, read commands, expire
/// timeouts, dispatch, and leave once stdin is closed and nothing is outstanding.
pub fn serve<B: ProbeBackend, W: Write>(
    helper: &mut Helper<B>,
    input: BorrowedFd<'_>,
    output: &mut W,
) -> Result<(), Fatal> {
    let mut buffer = CommandBuffer::new();
    let mut pipe_open = true;
    let mut responses: Vec<mtr_proto::Response> = Vec::new();
    let mut read_buf = vec![0u8; mtr_proto::COMMAND_BUFFER_SIZE];
    loop {
        // wait_for_activity() (wait_unix.c:109-155)
        {
            let mut fds: Vec<PollFd<'_>> = Vec::new();
            if pipe_open {
                fds.push(PollFd::new(input, PollFlags::POLLIN));
            }
            for fd in helper.backend.recv_fds() {
                fds.push(PollFd::new(fd, PollFlags::POLLIN));
            }
            for (_, fd) in helper.table.stream_fds() {
                fds.push(PollFd::new(fd, PollFlags::POLLOUT));
            }
            let timeout = match helper.table.next_timeout(Instant::now()) {
                Some(d) => PollTimeout::try_from(d).unwrap_or(PollTimeout::MAX),
                None => PollTimeout::NONE,
            };
            match poll(&mut fds, timeout) {
                Ok(_) => {}
                Err(nix::errno::Errno::EINTR | nix::errno::Errno::EAGAIN) => {}
                Err(e) => return Err(Fatal::Message(format!("unexpected select error: {e}"))),
            }
        }
        let now = Instant::now();
        helper
            .backend
            .receive(&mut helper.table, now, &mut responses);
        if pipe_open {
            let space = buffer.space_remaining();
            match nix::unistd::read(input, &mut read_buf[..space]) {
                Ok(0) => pipe_open = false,
                Ok(n) => buffer.push(&read_buf[..n]),
                Err(nix::errno::Errno::EAGAIN | nix::errno::Errno::EINTR) => {}
                Err(e) => {
                    return Err(Fatal::Message(format!(
                        "Unexpected command buffer read error: {e}"
                    )));
                }
            }
        }
        helper.table.expire(now, &mut responses);
        let (lines, overflow) = buffer.take_lines();
        for line in &lines {
            helper.dispatch_line(line, now, &mut responses);
        }
        if overflow {
            responses.push(mtr_proto::Response {
                token: 0,
                kind: mtr_proto::ResponseKind::CommandBufferOverflow,
            });
        }
        for r in responses.drain(..) {
            if output.write_all(r.encode().as_bytes()).is_err() {
                return Ok(()); // deviation 29: the client is gone
            }
        }
        if output.flush().is_err() {
            return Ok(());
        }
        if !pipe_open && helper.table.is_empty() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::fake::FakeBackend;
    use crate::command::Helper;
    use nix::unistd::{pipe, write};
    use std::os::fd::AsFd;

    #[test]
    fn serve_answers_commands_and_exits_on_eof_once_probes_are_done() {
        let (rd, wr) = pipe().unwrap();
        set_nonblocking(rd.as_fd()).unwrap();
        write(
            &wr,
            b"1 check-support feature ip-4\n2 send-probe ip-4 8.8.254.254 timeout 0\n3 send-pro",
        )
        .unwrap();
        write(&wr, b"be ip-4 127.0.0.1\n").unwrap();
        drop(wr); // EOF after the last command
        let mut fake = FakeBackend::v4_only();
        fake.reply_immediately = true; // the 127.0.0.1 probe gets a reply; the timeout-0 one expires
        let mut h = Helper::new(fake);
        let mut out = Vec::new();
        serve(&mut h, rd.as_fd(), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "1 feature-support support ok");
        assert!(
            lines.contains(&"2 no-reply") || lines.iter().any(|l| l.starts_with("2 reply")),
            "{text}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("3 reply ip-4 127.0.0.1 round-trip-time")),
            "{text}"
        );
        assert!(h.table.is_empty());
    }

    #[test]
    fn serve_reports_overflow_for_an_endless_line() {
        let (rd, wr) = pipe().unwrap();
        set_nonblocking(rd.as_fd()).unwrap();
        let big = vec![b'x'; 8192];
        write(&wr, &big).unwrap();
        write(&wr, b"\n").unwrap();
        drop(wr);
        let mut h = Helper::new(FakeBackend::v4_only());
        let mut out = Vec::new();
        serve(&mut h, rd.as_fd(), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("0 command-buffer-overflow\n"), "{text}");
    }
}
