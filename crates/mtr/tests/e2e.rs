use std::process::Command;

fn fake_helper() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-mtr-packet.py").to_string()
}

fn mtr_with_fake() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_mtr"));
    c.env_remove("MTR_OPTIONS").env("MTR_PACKET", fake_helper());
    c
}

fn run(c: &mut Command) -> (Option<i32>, String, String) {
    let o = c.output().unwrap();
    (
        o.status.code(),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

/// One cycle at the default 1 s interval: 4 probes 100 ms apart, then 0.2 s grace (~0.7 s).
const FAST: [&str; 6] = ["-n", "-c", "1", "-G", "0.2", "192.0.2.1"];

#[test]
fn report_with_the_fake_helper() {
    let (code, out, err) = run(mtr_with_fake().arg("-r").args(FAST));
    assert_eq!(code, Some(0), "stderr: {err}");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("Start: 20"), "{out}");
    assert!(lines[1].starts_with("HOST: "), "{out}");
    assert!(lines[2].starts_with("  1.|-- 10.0.0.1"), "{out}");
    assert!(lines[3].starts_with("  2.|-- 10.0.0.2"), "{out}");
    assert!(lines[4].starts_with("  3.|-- 192.0.2.1"), "{out}");
    assert_eq!(lines.len(), 5, "{out}");
    assert!(lines[2].contains("0.00%"), "{out}");
}

#[test]
fn wide_report_json_and_csv_with_the_fake_helper() {
    let (code, out, err) = run(mtr_with_fake().arg("-w").args(FAST));
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(out.contains("\n  3.|-- 192.0.2.1 "), "{out}");

    let (code, out, err) = run(mtr_with_fake().arg("-j").args(FAST));
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(out.starts_with("{\n    \"report\": {\n"), "{out}");
    assert!(
        out.contains("\"count\": 3,\n                \"host\": \"192.0.2.1\""),
        "{out}"
    );
    assert!(out.contains("\"tests\": 1,"), "{out}");

    let (code, out, err) = run(mtr_with_fake().arg("-C").args(FAST));
    assert_eq!(code, Some(0), "stderr: {err}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[0],
        "Mtr_Version,Start_Time,Status,Host,Hop,Ip,Loss%,Snt,,Last,Avg,Best,Wrst,StDev"
    );
    assert!(
        lines[3].starts_with(&format!("MTR.{},", env!("CARGO_PKG_VERSION"))),
        "{out}"
    );
    assert!(
        lines[3].contains(",OK,192.0.2.1,3,192.0.2.1,0.00,1,,3.00,3.00,3.00,3.00,0.00"),
        "{out}"
    );
}

#[test]
fn missing_helper_is_a_clear_fatal_error() {
    // helper::candidates() searches `<dir of current exe>/mtr-packet` first; running the
    // `mtr` binary straight out of `target/debug` would find the sibling mtr-packet built
    // by the workspace, defeating MTR_PACKET/PATH below. Run a copy from an empty temp
    // directory instead, so no sibling helper exists.
    let dir = std::env::temp_dir().join(format!(
        "mtr-e2e-missing-helper-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let mtr_copy = dir.join("mtr");
    std::fs::copy(env!("CARGO_BIN_EXE_mtr"), &mtr_copy).unwrap();

    let mut c = Command::new(&mtr_copy);
    c.env_remove("MTR_OPTIONS")
        .env("MTR_PACKET", "/nonexistent/mtr-packet")
        .env("PATH", "/nonexistent");
    let (code, out, err) = run(c.arg("-r").args(FAST));
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(code, Some(1));
    assert!(err.contains("mtr-packet not found"), "{err}");
    assert!(
        out.is_empty(),
        "no Start: line on the failure path: {out:?}"
    );
}

/// Real probes through the installed C helper: `MTR_E2E=1 cargo test -p mtr --test e2e -- --ignored`.
#[test]
#[ignore]
fn report_with_the_installed_c_helper() {
    if std::env::var_os("MTR_E2E").is_none() {
        return;
    }
    let mut c = Command::new(env!("CARGO_BIN_EXE_mtr"));
    c.env_remove("MTR_OPTIONS").env_remove("MTR_PACKET");
    let (code, out, err) = run(c.args(["-r", "-n", "-c", "1", "-G", "0.2", "127.0.0.1"]));
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(out.contains("  1.|-- 127.0.0.1"), "{out}");
    let mut c = Command::new(env!("CARGO_BIN_EXE_mtr"));
    c.env_remove("MTR_OPTIONS").env_remove("MTR_PACKET");
    let (code, out, _) = run(c.args(["-j", "-n", "-c", "1", "-G", "0.2", "127.0.0.1"]));
    assert_eq!(code, Some(0));
    assert!(out.contains("\"host\": \"127.0.0.1\""), "{out}");
}
