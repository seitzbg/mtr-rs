//! MILESTONE 3 (Task 15): run the real helper binary as a child and prove that by the time it
//! answers commands it holds no capabilities at all — packet.c:104-125 opens the sockets first
//! and then calls `drop_elevated_permissions()` (packet.c:44-102), so a `feature-support` reply
//! on stdout is proof that the drop already happened.
//!
//! Unprivileged this asserts the (already empty) sets stay empty; run against a copy carrying
//! `cap_net_raw+ep` via `MTR_PACKET_UNDER_TEST=/path/to/mtr-packet` and it proves the real
//! thing. GPL-2.0-only.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

/// The helper to exercise: the binary this test was built against, or the operator's setcap'd
/// copy named by `MTR_PACKET_UNDER_TEST`.
fn helper_path() -> String {
    std::env::var("MTR_PACKET_UNDER_TEST")
        .unwrap_or_else(|_| env!("CARGO_BIN_EXE_mtr-packet").to_string())
}

/// The `Cap*` lines of `/proc/<pid>/status`, as `(name, hex value)`.
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
fn the_running_helper_holds_no_capabilities() {
    let path = helper_path();
    let mut child = Command::new(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawning {path}: {e}"));
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    // Synchronisation point: the helper only reaches its command loop after `drop_all()`, so
    // reading this reply removes every timing assumption from the capability check below.
    stdin
        .write_all(b"1 check-support feature version\n")
        .unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert!(
        line.starts_with("1 feature-support support "),
        "unexpected reply {line:?}"
    );

    // The child is now blocked on stdin, which we still hold open.
    let caps = capabilities_of(child.id());
    for want in ["CapEff", "CapPrm", "CapInh"] {
        let (_, value) = caps
            .iter()
            .find(|(k, _)| k == want)
            .unwrap_or_else(|| panic!("no {want} in /proc/{}/status: {caps:?}", child.id()));
        assert_eq!(value, "0000000000000000", "{want} of {path} is not empty");
    }

    // Closing stdin ends the loop (lib.rs `serve`), as EOF does in packet.c:131-167.
    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success(), "{path} exited with {status}");
}
