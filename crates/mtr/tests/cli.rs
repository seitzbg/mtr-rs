use std::io::Write as _;
use std::process::{Command, Stdio};

fn mtr() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_mtr"));
    c.env_remove("MTR_OPTIONS");
    c
}

fn run(args: &[&str]) -> (Option<i32>, String, String) {
    let o = mtr().args(args).output().unwrap();
    (
        o.status.code(),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[test]
fn version_flag() {
    let (code, out, _) = run(&["-v"]);
    assert_eq!(code, Some(0));
    assert_eq!(out, format!("mtr {}\n", env!("CARGO_PKG_VERSION")));
    let (_, out, _) = run(&["-vv"]);
    assert!(out.contains("features:"));
}

#[test]
fn help_exits_zero_and_unknown_option_exits_one() {
    assert_eq!(run(&["--help"]).0, Some(0));
    assert_eq!(run(&["--frobnicate"]).0, Some(1));
}

#[test]
fn c_validation_messages_reach_stderr_with_exit_one() {
    let (code, _, err) = run(&["-u", "-T", "-r", "127.0.0.1"]);
    assert_eq!(code, Some(1));
    assert!(
        err.contains("mtr: -u , -T and -S are mutually exclusive"),
        "{err}"
    );
    let (code, _, err) = run(&["-P", "80", "-r", "127.0.0.1"]);
    assert_eq!(code, Some(1));
    assert!(
        err.contains("port number specified (-P) but protocol is ICMP"),
        "{err}"
    );
}

#[test]
fn mtr_options_is_read_but_the_command_line_wins() {
    let o = mtr()
        .env("MTR_OPTIONS", "-Q 999")
        .args(["-r", "127.0.0.1"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("value out of range (0 - 255): 999"));
    // -Q 5 overrides the environment; validation then fails later on the ICMP+port conflict
    let o = mtr()
        .env("MTR_OPTIONS", "-Q 999")
        .args(["-Q", "5", "-P", "80", "-r", "127.0.0.1"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&o.stderr).contains("protocol is ICMP"));
}

#[test]
fn interactive_mode_is_not_available_yet() {
    let (code, _, err) = run(&["127.0.0.1"]);
    assert_eq!(code, Some(1));
    assert!(
        err.contains("interactive mode is not implemented yet"),
        "{err}"
    );
}

#[test]
fn unresolvable_target_fails_with_c_message() {
    let (code, _, err) = run(&["-r", "no-such-host.invalid"]);
    assert_eq!(code, Some(1));
    assert!(
        err.contains("Failed to resolve host: no-such-host.invalid"),
        "{err}"
    );
}

#[test]
fn dash_f_dash_reads_hosts_from_stdin() {
    let mut child = mtr()
        .args(["-F", "-", "-r"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"no-such-host.invalid\n")
        .unwrap();
    let o = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    assert_eq!(o.status.code(), Some(1));
    assert!(
        err.contains("Failed to resolve host: no-such-host.invalid"),
        "{err}"
    );
}

#[test]
fn dash_f_names_precede_positional_names() {
    let path = std::env::temp_dir().join(format!("mtr-rs-cli-hosts-{}", std::process::id()));
    std::fs::write(&path, "a.invalid\n").unwrap();
    let (code, _, err) = run(&["-F", path.to_str().unwrap(), "-r", "b.invalid"]);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(code, Some(1), "{err}");
    let first = err
        .lines()
        .find(|l| l.contains("Failed to resolve host:"))
        .unwrap_or_else(|| panic!("no resolution failure in stderr: {err}"));
    assert!(first.contains("a.invalid"), "{err}");
}

#[test]
fn last_mode_flag_wins_like_getopt() {
    // `-j` then `-r`: report mode → prints "Start:" (JSON never does)
    let mut c = mtr();
    c.env(
        "MTR_PACKET",
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-mtr-packet.py"),
    );
    let o = c
        .args(["-j", "-r", "-n", "-c", "1", "-G", "0.2", "192.0.2.1"])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(o.status.code(), Some(0), "{out}");
    assert!(out.starts_with("Start: "), "{out}");
}
