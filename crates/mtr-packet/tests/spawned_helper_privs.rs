//! MILESTONE 3 (Task 15): run the real helper binary as a child and prove that by the time it
//! answers commands it holds no capabilities beyond a deliberately granted `CAP_NET_ADMIN` — packet.c:104-125 opens the sockets first
//! and then calls `drop_elevated_permissions()` (packet.c:44-102), so a `feature-support` reply
//! on stdout is proof that the drop already happened.
//!
//! Unprivileged this asserts the (already empty) sets stay empty; run against a copy carrying
//! `cap_net_raw+ep` via `MTR_PACKET_UNDER_TEST=/path/to/mtr-packet` and it proves the real
//! thing. Add `MTR_PACKET_EXPECT_NET_ADMIN=1` when that copy also carries `cap_net_admin+ep`,
//! and the exact surviving set is asserted. GPL-2.0-only.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// `CAP_NET_ADMIN` alone, as `/proc/<pid>/status` spells a capability mask: it is bit 12, so
/// 0x1000.
const NET_ADMIN_ONLY: &str = "0000000000001000";

/// Every wait in this test is bounded by this: a hung helper must fail the test, not hang
/// `cargo test --workspace` with no diagnostic.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The helper to exercise: the binary this test was built against, or the operator's setcap'd
/// copy named by `MTR_PACKET_UNDER_TEST`.
fn helper_path() -> String {
    std::env::var("MTR_PACKET_UNDER_TEST")
        .unwrap_or_else(|_| env!("CARGO_BIN_EXE_mtr-packet").to_string())
}

/// Owns the child for the whole test. `std::process::Child::drop` does *not* kill, so without
/// this a panicking assertion would leave a helper running; `Drop` here kills and reaps it.
struct Reaper(Child);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The `Cap*` lines of `/proc/<pid>/status`. That file is one `Name:\tvalue` pair per line
/// (tab-separated; `fs/proc/array.c`), and the capability sets appear as `CapInh:`, `CapPrm:`,
/// `CapEff:`, `CapBnd:` and `CapAmb:`, each a 16-digit hex mask. `status` stays world-readable
/// for a process that gained file capabilities (only `maps`/`mem`-style entries are gated on
/// ptrace access), so this also works against a setcap'd copy.
fn capabilities_of(pid: u32) -> Vec<(String, String)> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"))
        .unwrap_or_else(|e| panic!("reading /proc/{pid}/status: {e}"));
    status
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .filter(|(k, _)| k.starts_with("Cap") && k.ends_with(':'))
        .map(|(k, v)| (k.trim_end_matches(':').to_string(), v.trim().to_string()))
        .collect()
}

#[test]
fn the_running_helper_holds_no_capabilities_beyond_a_granted_net_admin() {
    let path = helper_path();
    let mut child = Command::new(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawning {path}: {e}"));
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut reaper = Reaper(child);
    let pid = reaper.0.id();

    // Synchronisation point: the helper only reaches its command loop after `drop_all()`, so
    // reading this reply removes every timing assumption from the capability check below.
    // The read runs on its own thread and the test waits on a channel, so the timeout holds
    // even for a stuck helper that handed its stdout to a grandchild — that pipe stays open
    // past `kill()`, and a plain `read_line` here would block forever.
    stdin
        .write_all(b"1 check-support feature version\n")
        .unwrap();
    stdin.flush().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = tx.send(stdout.read_line(&mut line).map(|_| line));
    });
    let line = match rx.recv_timeout(TIMEOUT) {
        Ok(Ok(line)) => line,
        Ok(Err(e)) => panic!("reading from {path}: {e}"),
        // `reaper` kills the child as this panic unwinds.
        Err(_) => panic!("{path} produced no reply within {TIMEOUT:?}"),
    };
    assert!(
        line.starts_with("1 feature-support support "),
        "unexpected reply {line:?} from {path}"
    );

    // The child is now blocked on stdin, which we still hold open. Deviation 34: `CAP_NET_ADMIN`
    // (bit 12, mask 0x1000) is the one capability the drop may keep, and only in the effective
    // and permitted sets, when the file capabilities granted it; the inheritable set is always
    // cleared.
    //
    // Unprivileged the test cannot tell which of the two outcomes to demand, so it accepts
    // either; set `MTR_PACKET_EXPECT_NET_ADMIN=1` when the helper under test really was given
    // the capability and the positive half of the deviation is asserted exactly, i.e. a helper
    // that wrongly dropped `CAP_NET_ADMIN` fails here.
    let caps = capabilities_of(pid);
    let value_of = |want: &str| {
        caps.iter()
            .find(|(k, _)| k == want)
            .unwrap_or_else(|| panic!("no {want} in /proc/{pid}/status: {caps:?}"))
            .1
            .clone()
    };
    assert_eq!(
        value_of("CapInh"),
        "0000000000000000",
        "CapInh of {path} is not empty"
    );
    // The ambient set is cleared explicitly in `drop_all()`; a non-empty one would survive an
    // `execve` of an unprivileged binary, so assert it in both modes.
    assert_eq!(
        value_of("CapAmb"),
        "0000000000000000",
        "CapAmb of {path} is not empty"
    );
    let expect_net_admin = std::env::var("MTR_PACKET_EXPECT_NET_ADMIN").is_ok_and(|v| v == "1");
    for want in ["CapEff", "CapPrm"] {
        let value = value_of(want);
        if expect_net_admin {
            assert_eq!(
                value, NET_ADMIN_ONLY,
                "{want} of {path} is {value}, expected cap_net_admin only \
                 (MTR_PACKET_EXPECT_NET_ADMIN=1)"
            );
        } else {
            assert!(
                value == "0000000000000000" || value == NET_ADMIN_ONLY,
                "{want} of {path} is {value}, expected empty or cap_net_admin only"
            );
        }
    }
    assert_eq!(
        value_of("CapEff"),
        value_of("CapPrm"),
        "effective and permitted sets of {path} disagree"
    );

    // Closing stdin ends the loop (lib.rs `serve`), as EOF does in packet.c:131-167. Poll for
    // the exit rather than blocking in `wait()`, so a helper that ignores EOF fails here too.
    drop(stdin);
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match reaper.0.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => panic!("waiting for {path}: {e}"),
        }
        assert!(
            Instant::now() < deadline,
            "{path} did not exit within {TIMEOUT:?} of stdin closing"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{path} exited with {status}");
}
