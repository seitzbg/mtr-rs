//! mtr-rs-packet: the privileged probe helper of mtr-rs. Rust port of mtr 0.96's `packet/`
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

/// The installed helper's name, used to prefix its diagnostics. The Rust port installs as
/// `mtr-rs-packet` so it can live beside mtr 0.96's `mtr-packet`.
pub const PROGRAM: &str = "mtr-rs-packet";

#[derive(Debug, thiserror::Error)]
pub enum Fatal {
    #[error("{0}")]
    Message(String),
    #[error("{0}: {1}")]
    Io(String, std::io::Error),
}

/// packet.c:104-125: open sockets privileged, drop privileges, finish init, serve stdin.
///
/// The drop happens before a single command is read, so every `check-support` answer — including
/// `mark`, which reports whether `CAP_NET_ADMIN` survived it (deviation 34) — describes the
/// unprivileged process that will actually send the probes.
pub fn run() -> Result<(), Fatal> {
    use std::os::fd::AsFd;
    let mut backend = backend::unix::UnixBackend::open_privileged()?;
    privs::drop_all()?;
    backend
        .finish_init()
        .map_err(|e| Fatal::Io("socket setup".into(), e))?;
    let stdin = std::io::stdin();
    set_nonblocking(stdin.as_fd())
        .map_err(|e| Fatal::Message(format!("Unexpected command stream error: {e}")))?;
    let mut helper = command::Helper::new(backend);
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    serve(&mut helper, stdin.as_fd(), &mut out)
}

/// `init_command_buffer()` / `set_socket_nonblocking()`: add `O_NONBLOCK`.
pub fn set_nonblocking(fd: BorrowedFd<'_>) -> nix::Result<()> {
    use nix::fcntl::{FcntlArg, OFlag, fcntl};
    let flags = OFlag::from_bits_truncate(fcntl(fd, FcntlArg::F_GETFL)?);
    fcntl(fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).map(|_| ())
}

/// Write one response. stdout is left **blocking** (only stdin gets `O_NONBLOCK`), exactly as
/// in C, so this normally never sees `WouldBlock`; but the fd is inherited from whoever spawned
/// us and may already carry `O_NONBLOCK`. Dropping the reply there would silently lose it, so
/// we retry the unwritten tail instead. Partial writes are tracked by hand: `write_all()`
/// restarts the whole slice after a `WouldBlock`, which would duplicate bytes on the wire.
fn write_blocking<W: Write>(out: &mut W, mut bytes: &[u8]) -> std::io::Result<()> {
    use std::io::ErrorKind;
    while !bytes.is_empty() {
        match out.write(bytes) {
            Ok(0) => return Err(ErrorKind::WriteZero.into()),
            Ok(n) => bytes = &bytes[n..],
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// `fflush(stdout)` (packet.c:133), with the same `WouldBlock` retry as `write_blocking`. A
/// `BufWriter` keeps whatever it could not write, so retrying the flush never duplicates.
fn flush_blocking<W: Write>(out: &mut W) -> std::io::Result<()> {
    use std::io::ErrorKind;
    loop {
        match out.flush() {
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            other => return other,
        }
    }
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
        if let Some(fatal) = helper.backend.take_fatal() {
            for r in responses.drain(..) {
                let _ = write_blocking(output, r.encode().as_bytes());
            }
            let _ = flush_blocking(output);
            return Err(fatal);
        }
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
            if write_blocking(output, r.encode().as_bytes()).is_err() {
                return Ok(()); // deviation 29: the client is gone
            }
        }
        if flush_blocking(output).is_err() {
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

    /// A fatal receive error must not lose the responses `receive()` produced in the same
    /// call: C exits from `receive_replies()` only after its `printf`s have gone out
    /// (probe_unix.c:790), so `serve()` flushes first and then returns `Fatal`.
    #[test]
    fn a_fatal_receive_error_flushes_pending_responses_first() {
        let (rd, wr) = pipe().unwrap();
        set_nonblocking(rd.as_fd()).unwrap();
        write(&wr, b"7 send-probe ip-4 127.0.0.1\n").unwrap();
        let mut fake = FakeBackend::v4_only();
        fake.reply_immediately = true;
        fake.fail_receive = Some(nix::libc::ENOBUFS);
        let mut h = Helper::new(fake);
        let mut out = Vec::new();
        let e = serve(&mut h, rd.as_fd(), &mut out).unwrap_err();
        drop(wr);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.starts_with("7 reply ip-4 127.0.0.1 round-trip-time"),
            "{text}"
        );
        assert!(
            e.to_string()
                .starts_with("Failure receiving replies: No buffer space"),
            "{e}"
        );
    }

    #[test]
    fn serve_reports_overflow_for_an_endless_line() {
        let (rd, wr) = pipe().unwrap();
        set_nonblocking(rd.as_fd()).unwrap();
        // Written from another thread: FreeBSD hands a pipe write of `kern.ipc.pipe_mindirect`
        // (8192) bytes or more straight to the reader and blocks until it has been read, so
        // writing it here before `serve()` starts reading would deadlock the test.
        let writer = std::thread::spawn(move || {
            let big = vec![b'x'; 8192];
            write(&wr, &big).unwrap();
            write(&wr, b"\n").unwrap();
            drop(wr);
        });
        let mut h = Helper::new(FakeBackend::v4_only());
        let mut out = Vec::new();
        serve(&mut h, rd.as_fd(), &mut out).unwrap();
        writer.join().unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("0 command-buffer-overflow\n"), "{text}");
    }
}
