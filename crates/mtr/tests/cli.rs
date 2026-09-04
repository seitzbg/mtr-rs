use std::io::Write as _;
use std::process::{Command, Stdio};

fn mtr() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_mtr"));
    // Point the config file at a directory that cannot exist, so these tests never pick up the
    // developer's own ~/.config/mtr-rs/config.toml.
    c.env_remove("MTR_OPTIONS")
        .env("XDG_CONFIG_HOME", "/nonexistent/mtr-rs-tests");
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

/// A per-test config directory; `--config` points the binary straight at the file.
fn temp_config(name: &str, text: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mtr-rs-cli-cfg-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    if !text.is_empty() {
        std::fs::write(&path, text).unwrap();
    }
    path
}

#[test]
fn a_malformed_config_file_is_fatal_with_its_path_and_line() {
    let path = temp_config("malformed", "[display]\nascii = true\nnope\n");
    let (code, _, err) = run(&["--config", path.to_str().unwrap(), "-r", "127.0.0.1"]);
    assert_eq!(code, Some(1), "{err}");
    assert!(
        err.starts_with(&format!("mtr: config: {}: ", path.display())),
        "{err}"
    );
    assert!(err.contains("line 3"), "{err}");
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn an_invalid_config_value_reports_the_cli_validation_message() {
    let path = temp_config("invalid", "[display]\nfields = \"LSQ\"\n");
    let (code, _, err) = run(&["--config", path.to_str().unwrap(), "-r", "127.0.0.1"]);
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("Unknown field identifier: Q"), "{err}");
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn a_missing_config_file_is_not_an_error() {
    let path = temp_config("absent", "");
    let (code, _, err) = run(&[
        "--config",
        path.to_str().unwrap(),
        "-r",
        "no-such-host.invalid",
    ]);
    assert_eq!(code, Some(1));
    assert!(err.contains("Failed to resolve host"), "{err}");
    assert!(!err.contains("config:"), "{err}");
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn the_config_file_supplies_defaults_the_command_line_still_overrides() {
    // interval 0.5 from the file trips the non-root check, proving the value was applied…
    let path = temp_config("interval", "[probe]\ninterval = 0.5\n");
    let (code, _, err) = run(&["--config", path.to_str().unwrap(), "-r", "127.0.0.1"]);
    assert_eq!(code, Some(1), "{err}");
    assert!(
        err.contains("non-root users cannot request an interval < 1.0"),
        "{err}"
    );
    // …and -i 1 on the command line replaces it, so the run gets as far as resolution
    let (_, _, err) = run(&[
        "--config",
        path.to_str().unwrap(),
        "-i",
        "1",
        "-r",
        "no-such-host.invalid",
    ]);
    assert!(err.contains("Failed to resolve host"), "{err}");
    // $MTR_OPTIONS beats the file for the same reason
    let o = mtr()
        .env("MTR_OPTIONS", "-i 1")
        .args([
            "--config",
            path.to_str().unwrap(),
            "-r",
            "no-such-host.invalid",
        ])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&o.stderr).contains("Failed to resolve host"));
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn init_config_writes_the_file_once_and_prints_its_path() {
    let path = temp_config("init", "");
    let arg = path.to_str().unwrap();
    let (code, out, err) = run(&["--init-config", "--config", arg]);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(out, format!("{}\n", path.display()));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        include_str!("../../../docs/config.example.toml")
    );
    // a second run refuses rather than clobbering it
    let (code, _, err) = run(&["--init-config", "--config", arg]);
    assert_eq!(code, Some(1));
    assert!(
        err.contains("file exists, refusing to overwrite it"),
        "{err}"
    );
    // and what it wrote is loadable
    let (_, _, err) = run(&["--config", arg, "-r", "no-such-host.invalid"]);
    assert!(err.contains("Failed to resolve host"), "{err}");
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn init_config_creates_missing_parent_directories() {
    let dir = std::env::temp_dir().join(format!("mtr-rs-cli-cfg-{}-mkdir", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("a").join("b").join("config.toml");
    let (code, out, err) = run(&["--init-config", "--config", path.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(out.trim_end(), path.to_str().unwrap());
    assert!(path.is_file());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A private `$XDG_CONFIG_HOME`, so these tests exercise the *default* path resolution rather
/// than `--config`.
fn temp_xdg(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mtr-rs-cli-xdg-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn init_config_uses_the_xdg_default_path_when_no_config_flag_is_given() {
    let xdg = temp_xdg("init");
    let expected = xdg.join("mtr-rs").join("config.toml");
    let o = mtr()
        .env("XDG_CONFIG_HOME", &xdg)
        .arg("--init-config")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{:?}", o.stderr);
    assert_eq!(
        String::from_utf8_lossy(&o.stdout),
        format!("{}\n", expected.display())
    );
    assert_eq!(
        std::fs::read_to_string(&expected).unwrap(),
        include_str!("../../../docs/config.example.toml")
    );
    std::fs::remove_dir_all(&xdg).unwrap();
}

#[test]
fn the_xdg_default_config_is_read_without_a_config_flag() {
    let xdg = temp_xdg("read");
    let path = xdg.join("mtr-rs").join("config.toml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[display]\nfields = \"LS\"\n").unwrap();
    let o = mtr()
        .env("XDG_CONFIG_HOME", &xdg)
        .env(
            "MTR_PACKET",
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-mtr-packet.py"),
        )
        .args(["-r", "-n", "-c", "1", "-G", "0.2", "192.0.2.1"])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(o.status.code(), Some(0), "{out}");
    // `fields = "LS"` from the file: Loss% and Snt, and none of the default's later columns
    let header = out.lines().find(|l| l.starts_with("HOST:")).unwrap();
    assert!(header.ends_with("Loss%   Snt"), "{header}");
    std::fs::remove_dir_all(&xdg).unwrap();
}

#[test]
fn a_bad_default_config_is_fatal_too() {
    let xdg = temp_xdg("bad");
    let path = xdg.join("mtr-rs").join("config.toml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[probe]\nmax_ttl = 0\n").unwrap();
    let o = mtr()
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["-r", "127.0.0.1"])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert!(
        err.contains(&format!(
            "mtr: config: {}: value out of range (1 - 255): 0",
            path.display()
        )),
        "{err}"
    );
    std::fs::remove_dir_all(&xdg).unwrap();
}

#[test]
fn color_never_in_the_file_can_be_overridden_from_the_command_line() {
    // `--color`'s effect is not observable in report mode, so assert the flags parse and that the
    // run still gets as far as resolution — the merge itself is unit-tested in config_file.
    let path = temp_config("color", "[display]\ncolor = \"never\"\n");
    let (_, _, err) = run(&[
        "--config",
        path.to_str().unwrap(),
        "--color",
        "always",
        "-r",
        "no-such-host.invalid",
    ]);
    assert!(err.contains("Failed to resolve host"), "{err}");
    let (code, _, err) = run(&["--color", "sometimes", "-r", "127.0.0.1"]);
    assert_eq!(code, Some(1));
    assert!(err.contains("invalid value 'sometimes'"), "{err}");
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn report_mode_runs_every_target_and_keeps_going_after_a_failure() {
    let mut c = mtr();
    c.env(
        "MTR_PACKET",
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-mtr-packet.py"),
    );
    let o = c
        .args(["-r", "-n", "-c", "1", "-G", "0.2", "192.0.2.1", "192.0.2.2"])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(o.status.code(), Some(0), "{out}");
    assert_eq!(out.matches("Start: ").count(), 2, "{out}");
}

#[test]
fn json_with_two_targets_is_a_single_array() {
    let mut c = mtr();
    c.env(
        "MTR_PACKET",
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-mtr-packet.py"),
    );
    let o = c
        .args(["-j", "-n", "-c", "1", "-G", "0.2", "192.0.2.1", "192.0.2.2"])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(o.status.code(), Some(0), "{out}");
    assert!(out.starts_with("[\n"), "{out}");
    assert!(out.trim_end().ends_with(']'), "{out}");
    assert_eq!(out.matches("\"report\"").count(), 2, "{out}");
}
