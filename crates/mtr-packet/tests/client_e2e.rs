//! Both mtr clients driving our helper. GPL-2.0-only.
//! `cargo build --workspace && MTR_E2E=1 cargo test -p mtr-packet --test client_e2e -- --ignored`.
use std::path::Path;
use std::process::Command;

fn our_helper() -> &'static str {
    env!("CARGO_BIN_EXE_mtr-rs-packet")
}

fn our_client() -> std::path::PathBuf {
    Path::new(our_helper()).with_file_name("mtr-rs")
}

fn run(mut c: Command) -> (Option<i32>, String, String) {
    let o = c.output().unwrap();
    (
        o.status.code(),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[test]
#[ignore]
fn our_client_reports_through_our_helper() {
    if std::env::var_os("MTR_E2E").is_none() {
        return;
    }
    assert!(
        our_client().exists(),
        "build the workspace first: cargo build --workspace"
    );
    let mut c = Command::new(our_client());
    c.env_remove("MTR_OPTIONS")
        .env("MTR_PACKET", our_helper())
        .args(["-r", "-n", "-c", "1", "-G", "0.2", "127.0.0.1"]);
    let (code, out, err) = run(c);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(out.contains("  1.|-- 127.0.0.1"), "{out}");
    let mut c = Command::new(our_client());
    c.env_remove("MTR_OPTIONS")
        .env("MTR_PACKET", our_helper())
        .args(["-j", "-n", "-u", "-c", "1", "-G", "0.2", "127.0.0.1"]);
    let (code, out, _) = run(c);
    assert_eq!(code, Some(0));
    assert!(out.contains("\"host\": \"127.0.0.1\""), "{out}");
}

#[test]
#[ignore]
fn the_c_client_reports_through_our_helper() {
    if std::env::var_os("MTR_E2E").is_none() || !Path::new("/usr/bin/mtr").exists() {
        return;
    }
    if Path::new("/etc/mtr.is.run.under.sudo").exists() {
        eprintln!("skipping: the sudo guard file makes the C client ignore MTR_PACKET");
        return;
    }
    let mut c = Command::new("/usr/bin/mtr");
    c.env_remove("MTR_OPTIONS")
        .env("MTR_PACKET", our_helper())
        .args(["-r", "-n", "-c", "1", "127.0.0.1"]);
    let (code, out, err) = run(c);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert!(out.contains("127.0.0.1"), "{out}");
}
