use std::process::Command;

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
