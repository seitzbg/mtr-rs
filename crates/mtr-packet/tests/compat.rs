//! Runs mtr's upstream helper suites against our binary. GPL-2.0-only.
//!
//! `cmdparse.py TestCommandParse` is the unprivileged acceptance class (the sibling
//! `TestMtrCommandParse` drives the *C* client binary, which this workspace does not build).
//! `param.py` and `probe.py` need `cap_net_raw` on the helper, and `param.py` additionally
//! needs it on `$MTR_C_REPO/test/mtr-packet-listen`, so they are `#[ignore]`d and gated on
//! `MTR_E2E=1`.
//!
//! Known failures that reproduce identically against the `/usr/bin/mtr-packet` baseline and so
//! are excluded from acceptance -- `tests/compat/run.sh --compare` is the machine-checked
//! version of this list, comparing the two helpers failure-id by failure-id:
//!   * `probe.py` `TestProbeICMPv6.test_probe`, `TestProbeICMPv6.test_ttl_expired`,
//!     `TestProbeUDP.test_udp_v6`, `TestProbeTCP.test_tcp_v6`, `TestProbeSCTP.test_sctp_v6`
//!     -- this box has no usable IPv6 path, so the C helper fails them too.
//!   * `probe.py` `TestProbeICMPv4.test_exhaust_probes` may join that list: it opens 4096
//!     concurrent probes against the helper's `MAX_PROBES` of 10240, so it is bounded by the
//!     process file-descriptor limit rather than by the helper.
use std::path::{Path, PathBuf};
use std::process::Command;

fn runner() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/run.sh")
}

fn c_repo() -> Option<PathBuf> {
    let p = std::env::var_os("MTR_C_REPO")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("git/mtr")))?;
    p.join("test/probe.py").exists().then_some(p)
}

fn have_python3() -> bool {
    Command::new("python3")
        .arg("-V")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn run_suite(suite: &str, args: &[&str]) -> (bool, String) {
    let repo = c_repo().expect("C repo present (checked by caller)");
    let out = Command::new(runner())
        .arg(suite)
        .args(args)
        .env("MTR_PACKET", env!("CARGO_BIN_EXE_mtr-packet"))
        .env("MTR_C_REPO", &repo)
        .output()
        .expect("bash + python3 available");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn cmdparse_helper_class_passes_unprivileged() {
    if c_repo().is_none() || !have_python3() {
        return;
    }
    let (ok, text) = run_suite("cmdparse", &["TestCommandParse"]);
    assert!(ok, "{text}");
    assert!(text.contains("Ran 6 tests"), "{text}");
}

/// Needs cap_net_raw on the helper and on test/mtr-packet-listen, plus network:
/// `MTR_E2E=1 cargo test -p mtr-packet --test compat -- --ignored`.
///
/// Runs each suite twice -- once against our helper, once against `/usr/bin/mtr-packet` -- and
/// only fails on test ids that fail for us and pass for the C helper, per the doc comment
/// above. `param.py` reports itself skipped when `test/mtr-packet-listen` has no capability.
#[test]
#[ignore]
fn param_and_probe_suites_pass_with_cap_net_raw() {
    if std::env::var_os("MTR_E2E").is_none() || c_repo().is_none() || !have_python3() {
        return;
    }
    let (ok, text) = run_suite("--compare", &["param", "probe"]);
    println!("{text}");
    assert!(ok, "failures the C helper does not have:\n{text}");
}
