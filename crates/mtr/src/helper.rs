//! The `mtr-rs-packet` child: search order and spawn (ui/cmdpipe.c:240-372), startup handshake
//! (ui/cmdpipe.c:181-220), reply plumbing (ui/cmdpipe.c:690-917) — mtr 0.96, commit 7b01773.
//! GPL-2.0-only.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use mtr_proto::{Feature, Protocol, Request, RequestKind, Response, ResponseKind};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;

/// Our privileged probe helper. The C helper, `mtr-packet`, stays a last-resort fallback in
/// [`candidates_from`] because it speaks the same wire protocol.
pub const HELPER: &str = "mtr-rs-packet";

/// mtr 0.96's helper: the final fallback when no `mtr-rs-packet` is installed.
pub const C_HELPER: &str = "mtr-packet";

#[derive(Debug, Error)]
pub enum HelperError {
    #[error("mtr-rs-packet not found; tried: {}", .0.join(", "))]
    NotFound(Vec<String>),
    #[error(
        "mtr-rs-packet did not answer the startup check; check that it is installed and allowed to open probe sockets"
    )]
    StartupCheck,
    #[error("{}", unsupported_message(*.0))]
    Unsupported(Feature),
    #[error("mtr-rs-packet command pipe failure: {0}")]
    Io(#[from] std::io::Error),
}

/// C prints "Packet type unsupported" for every unsupported feature. Deviation 34: `mark` is
/// different in kind — on Linux the helper is installed but simply lacks `CAP_NET_ADMIN` — so
/// say what is missing and how to grant it. Elsewhere `SO_MARK` does not exist, and the CLI
/// already refuses `-M`; this text is for a helper reached some other way.
fn unsupported_message(feature: Feature) -> String {
    match feature {
        Feature::Mark if cfg!(target_os = "linux") => {
            "mtr-rs-packet does not support --mark here: grant it cap_net_admin \
             (sudo setcap cap_net_raw,cap_net_admin+ep \"$(command -v mtr-rs-packet)\")"
                .to_string()
        }
        Feature::Mark => {
            "mtr-rs-packet does not support --mark on this platform (SO_MARK is Linux only)"
                .to_string()
        }
        f => format!("Packet type unsupported: {}", f.as_str()),
    }
}

#[derive(Debug)]
pub enum HelperEvent {
    Response(Response),
    /// The helper's stdout closed: it exited. Fatal, as in C ("unexpected packet generator exit").
    Exited,
}

/// A running helper. Dropping it kills the child (`kill_on_drop`).
///
/// The request channel (`tx`) holds 256 entries and the event channel (`rx`) holds 1024. The
/// engine issues at most one probe per tick, and the driver drains `rx` on every `select!`
/// iteration, so under the current driver neither side can fill up. Any future driver
/// implementation must keep draining `rx` while it awaits `tx.send`, or the helper's stdout
/// back-pressure (it blocks writing replies once `ev_tx` is full) can stall the whole pipeline.
pub struct Helper {
    pub tx: mpsc::Sender<Request>,
    pub rx: mpsc::Receiver<HelperEvent>,
    _child: Child,
}

/// Path to the marker file that indicates mtr is running under `sudo` (`ui/mtr.c:717-721` and
/// `execute_packet_child()`). When present, every caller-controlled filesystem path is refused
/// and helper discovery uses absolute, installation-controlled paths only.
pub const SUDO_GUARD_FILE: &str = "/etc/mtr.is.run.under.sudo";

/// Fixed helper locations used under the sudo guard. The `bin` entries cover this project's
/// source, Debian and FreeBSD installs; the `sbin` entries preserve fallback to distributions
/// that classify the C helper as an administration command.
const SYSTEM_HELPER_DIRS: [&str; 4] =
    ["/usr/local/bin", "/usr/local/sbin", "/usr/bin", "/usr/sbin"];

/// Whether the sudo marker file exists.
pub fn sudo_guard_present() -> bool {
    Path::new(SUDO_GUARD_FILE).exists()
}

/// `execute_packet_child()`: ordinarily `$MTR_PACKET`, `mtr-rs-packet` on `PATH`, next to our own
/// executable, `./mtr-rs-packet`, and finally the C `mtr-packet` on `PATH`. Under the sudo guard,
/// `$MTR_PACKET`, `PATH` and the current directory are all untrusted, so only absolute paths next
/// to this executable and in [`SYSTEM_HELPER_DIRS`] are returned.
pub fn candidates() -> Vec<PathBuf> {
    candidates_from(
        std::env::var_os("MTR_PACKET"),
        sudo_guard_present(),
        std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf)),
    )
}

/// Pure core of [`candidates`]: the normal C-compatible search order, or the absolute-only guarded
/// order, given explicit inputs and without touching the environment or filesystem. The C helper
/// (`mtr-packet`) comes last, after every `mtr-rs-packet` location.
pub fn candidates_from(
    mtr_packet_env: Option<std::ffi::OsString>,
    guard_present: bool,
    exe_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let push_unique = |v: &mut Vec<PathBuf>, p: PathBuf| {
        if !v.contains(&p) {
            v.push(p);
        }
    };
    if guard_present {
        let exe_dir = exe_dir.filter(|p| p.is_absolute());
        if let Some(dir) = &exe_dir {
            push_unique(&mut v, dir.join(HELPER));
        }
        for dir in SYSTEM_HELPER_DIRS {
            push_unique(&mut v, Path::new(dir).join(HELPER));
        }
        if let Some(dir) = &exe_dir {
            push_unique(&mut v, dir.join(C_HELPER));
        }
        for dir in SYSTEM_HELPER_DIRS {
            push_unique(&mut v, Path::new(dir).join(C_HELPER));
        }
        return v;
    }
    if let Some(p) = mtr_packet_env {
        v.push(PathBuf::from(p));
    }
    v.push(PathBuf::from(HELPER));
    if let Some(dir) = exe_dir {
        v.push(dir.join(HELPER));
    }
    v.push(PathBuf::from(format!("./{HELPER}")));
    // Last resort: mtr 0.96's own helper speaks the identical wire protocol, so a system that
    // only has the distribution's mtr-packet still works.
    v.push(PathBuf::from(C_HELPER));
    v
}

pub async fn spawn(want_v6: bool, protocol: Protocol, mark: u32) -> Result<Helper, HelperError> {
    spawn_with(&candidates(), want_v6, protocol, mark).await
}

pub async fn spawn_with(
    paths: &[PathBuf],
    want_v6: bool,
    protocol: Protocol,
    mark: u32,
) -> Result<Helper, HelperError> {
    let mut child = None;
    for p in paths {
        match Command::new(p)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(c) => {
                child = Some(c);
                break;
            }
            Err(e) => tracing::debug!("spawn {}: {e}", p.display()),
        }
    }
    let mut child = child.ok_or_else(|| {
        HelperError::NotFound(paths.iter().map(|p| p.display().to_string()).collect())
    })?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut lines = BufReader::new(child.stdout.take().expect("piped stdout")).lines();

    // Handshake (cmdpipe.c:181-220), every request with token 1.
    if !check(&mut stdin, &mut lines, Feature::SendProbe).await? {
        return Err(HelperError::StartupCheck);
    }
    let mut required = vec![
        if want_v6 { Feature::Ip6 } else { Feature::Ip4 },
        Feature::for_protocol(protocol),
    ];
    if mark != 0 {
        required.push(Feature::Mark);
    }
    for f in required {
        if !check(&mut stdin, &mut lines, f).await? {
            return Err(HelperError::Unsupported(f));
        }
    }

    let (req_tx, mut req_rx) = mpsc::channel::<Request>(256);
    let (ev_tx, ev_rx) = mpsc::channel::<HelperEvent>(1024);
    tokio::spawn(async move {
        while let Some(r) = req_rx.recv().await {
            if let Err(e) = stdin.write_all(r.encode().as_bytes()).await {
                tracing::error!("mtr-rs-packet command pipe write failure: {e}");
                break;
            }
        }
    });
    tokio::spawn(async move {
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => match Response::parse(&line) {
                    Ok(resp) => {
                        if ev_tx.send(HelperEvent::Response(resp)).await.is_err() {
                            break;
                        }
                    }
                    // Deviation 9: C dies with "reply parse failure"; we log and go on.
                    Err(e) => tracing::warn!("unparsable reply from the helper {line:?}: {e}"),
                },
                Ok(None) | Err(_) => {
                    let _ = ev_tx.send(HelperEvent::Exited).await;
                    break;
                }
            }
        }
    });
    Ok(Helper {
        tx: req_tx,
        rx: ev_rx,
        _child: child,
    })
}

/// One `check-support` exchange; `Ok(true)` iff the reply is `feature-support support ok`.
async fn check(
    stdin: &mut ChildStdin,
    lines: &mut Lines<BufReader<ChildStdout>>,
    feature: Feature,
) -> Result<bool, HelperError> {
    let req = Request {
        token: 1,
        kind: RequestKind::CheckSupport { feature },
    };
    stdin.write_all(req.encode().as_bytes()).await?;
    let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .map_err(|_| HelperError::StartupCheck)??
        .ok_or(HelperError::StartupCheck)?;
    match Response::parse(&line) {
        Ok(Response {
            kind: ResponseKind::FeatureSupport(v),
            ..
        }) => Ok(v == "ok"),
        _ => Err(HelperError::StartupCheck),
    }
}

/// The two ways to let the helper open probe sockets, for the failures that mean it could not:
/// the `permission-denied` fatal, the startup check (the helper died opening its sockets) and an
/// unsupported `ip-4`/`ip-6` (that family's socket never opened). On Linux, without `cap_net_raw`
/// the helper falls back to unprivileged DGRAM ICMP, which the kernel only allows to gids inside
/// `net.ipv4.ping_group_range` — so `setcap` alone is not the whole story. FreeBSD and macOS have
/// no capabilities and no fallback: raw sockets need root, so the helper is installed setuid root.
pub fn privilege_hint(err: &str) -> Option<String> {
    let socket_failure = err == fatal_message(&ResponseKind::PermissionDenied)?
        || err == HelperError::StartupCheck.to_string()
        || err == HelperError::Unsupported(Feature::Ip4).to_string()
        || err == HelperError::Unsupported(Feature::Ip6).to_string();
    if !socket_failure {
        return None;
    }
    if cfg!(target_os = "linux") {
        Some(format!(
            "hint: raw sockets need a capability: sudo setcap cap_net_raw+ep \"$(command -v {HELPER})\"\n\
             hint: or allow unprivileged ICMP for your group: sudo sysctl -w net.ipv4.ping_group_range=\"0 2147483647\""
        ))
    } else {
        Some(format!(
            "hint: raw sockets need root: sudo chown root \"$(command -v {HELPER})\" && sudo chmod u+s \"$(command -v {HELPER})\"\n\
             hint: or run mtr-rs itself as root"
        ))
    }
}

/// The replies `handle_reply_errors()` (cmdpipe.c:690-728) treats as fatal, with its messages.
pub fn fatal_message(kind: &ResponseKind) -> Option<&'static str> {
    Some(match kind {
        ResponseKind::ProbesExhausted => "Probes exhausted",
        ResponseKind::InvalidArgument { .. } => "mtr-rs-packet reported invalid argument",
        ResponseKind::PermissionDenied => {
            "mtr-rs-packet reported permission denied while sending a probe"
        }
        ResponseKind::AddressInUse => "Address in use",
        ResponseKind::AddressNotAvailable => "Address not available",
        ResponseKind::UnexpectedError { .. } => "Unexpected mtr-rs-packet error",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtr_proto::{ProbeParams, ProbeResult};

    fn fake() -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fake-mtr-packet.py"
        ))
    }

    #[tokio::test]
    async fn handshake_and_probe_round_trip_with_the_fake_helper() {
        let mut h = spawn_with(&[fake()], false, Protocol::Icmp, 0)
            .await
            .unwrap();
        let mut p = ProbeParams::new("192.0.2.1".parse().unwrap());
        p.ttl = Some(1);
        h.tx.send(Request {
            token: 33000,
            kind: RequestKind::SendProbe(p),
        })
        .await
        .unwrap();
        match h.rx.recv().await {
            Some(HelperEvent::Response(r)) => {
                assert_eq!(r.token, 33000);
                assert!(matches!(
                    r.kind,
                    ResponseKind::Probe {
                        result: ProbeResult::TtlExpired,
                        rtt_us: 1000,
                        ..
                    }
                ));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn unsupported_feature_is_reported_like_packet_type_unsupported() {
        let err = spawn_with(&[fake()], false, Protocol::Sctp, 0)
            .await
            .err()
            .unwrap();
        assert!(matches!(err, HelperError::Unsupported(Feature::Sctp)));
        assert_eq!(err.to_string(), "Packet type unsupported: sctp");
    }

    #[test]
    fn unsupported_mark_explains_the_missing_capability() {
        // Deviation 34: `mark` is only supported when the helper holds CAP_NET_ADMIN, so the
        // message has to say how to grant it rather than just "packet type unsupported". Off
        // Linux there is nothing to grant, and the message says so instead.
        let msg = HelperError::Unsupported(Feature::Mark).to_string();
        if cfg!(target_os = "linux") {
            assert!(msg.contains("cap_net_admin"), "{msg}");
        } else {
            assert!(msg.contains("Linux only"), "{msg}");
        }
        assert!(msg.contains("--mark"), "{msg}");
    }

    #[tokio::test]
    async fn missing_helper_lists_the_candidates() {
        let err = spawn_with(
            &[PathBuf::from("/nonexistent/mtr-rs-packet")],
            false,
            Protocol::Icmp,
            0,
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(err, HelperError::NotFound(_)));
        assert!(err.to_string().contains("/nonexistent/mtr-rs-packet"));
    }

    #[tokio::test]
    async fn helper_exit_is_reported() {
        let h = spawn_with(&[fake()], false, Protocol::Icmp, 0)
            .await
            .unwrap();
        let Helper { tx, mut rx, .. } = h;
        drop(tx); // closes the helper's stdin -> the fake exits -> stdout EOF
        assert!(matches!(rx.recv().await, Some(HelperEvent::Exited)));
    }

    #[test]
    fn candidate_order_matches_execute_packet_child() {
        let c = candidates();
        if sudo_guard_present() {
            assert!(c.iter().all(|p| p.is_absolute()), "{c:?}");
        } else {
            assert_eq!(c.last().unwrap(), &PathBuf::from("mtr-packet"));
            assert!(c.iter().any(|p| p == &PathBuf::from(HELPER)));
        }
    }

    #[test]
    fn candidates_from_full_order_with_env_and_exe_dir() {
        let c = candidates_from(
            Some(std::ffi::OsString::from("/opt/mtr-rs-packet")),
            false,
            Some(PathBuf::from("/usr/local/bin")),
        );
        assert_eq!(
            c,
            vec![
                PathBuf::from("/opt/mtr-rs-packet"),
                PathBuf::from("mtr-rs-packet"),
                PathBuf::from("/usr/local/bin/mtr-rs-packet"),
                PathBuf::from("./mtr-rs-packet"),
                PathBuf::from("mtr-packet"),
            ]
        );
    }

    #[test]
    fn candidates_from_guard_present_uses_only_absolute_trusted_locations() {
        let c = candidates_from(
            Some(std::ffi::OsString::from("/opt/mtr-rs-packet")),
            true,
            Some(PathBuf::from("/opt/mtr/bin")),
        );
        assert!(!c.contains(&PathBuf::from("/opt/mtr-rs-packet")));
        assert_eq!(
            c.first().unwrap(),
            &PathBuf::from("/opt/mtr/bin/mtr-rs-packet")
        );
        assert!(c.iter().all(|p| p.is_absolute()), "{c:?}");
        assert!(!c.contains(&PathBuf::from("mtr-rs-packet")));
        assert!(!c.contains(&PathBuf::from("./mtr-rs-packet")));
        assert_eq!(
            c,
            vec![
                PathBuf::from("/opt/mtr/bin/mtr-rs-packet"),
                PathBuf::from("/usr/local/bin/mtr-rs-packet"),
                PathBuf::from("/usr/local/sbin/mtr-rs-packet"),
                PathBuf::from("/usr/bin/mtr-rs-packet"),
                PathBuf::from("/usr/sbin/mtr-rs-packet"),
                PathBuf::from("/opt/mtr/bin/mtr-packet"),
                PathBuf::from("/usr/local/bin/mtr-packet"),
                PathBuf::from("/usr/local/sbin/mtr-packet"),
                PathBuf::from("/usr/bin/mtr-packet"),
                PathBuf::from("/usr/sbin/mtr-packet"),
            ]
        );
    }

    #[test]
    fn guarded_candidates_ignore_a_relative_executable_directory() {
        let c = candidates_from(None, true, Some(PathBuf::from("relative/bin")));
        assert!(c.iter().all(|p| p.is_absolute()), "{c:?}");
        assert_eq!(
            c.first().unwrap(),
            &PathBuf::from("/usr/local/bin/mtr-rs-packet")
        );
    }

    #[test]
    fn candidates_from_env_unset_leads_with_our_helper() {
        let c = candidates_from(None, false, None);
        assert_eq!(
            c,
            vec![
                PathBuf::from("mtr-rs-packet"),
                PathBuf::from("./mtr-rs-packet"),
                PathBuf::from("mtr-packet"),
            ]
        );
    }

    /// The C helper is the last resort, after every mtr-rs-packet location.
    #[test]
    fn the_c_helper_is_the_final_fallback() {
        let c = candidates_from(None, false, Some(PathBuf::from("/usr/local/bin")));
        assert_eq!(c.last().unwrap(), &PathBuf::from("mtr-packet"));
        assert_eq!(
            c.iter().position(|p| p == &PathBuf::from("mtr-packet")),
            Some(c.len() - 1)
        );
    }

    #[test]
    fn privilege_hint_names_both_fixes_for_every_socket_failure() {
        for msg in [
            fatal_message(&ResponseKind::PermissionDenied)
                .unwrap()
                .to_string(),
            HelperError::StartupCheck.to_string(),
            HelperError::Unsupported(Feature::Ip4).to_string(),
            HelperError::Unsupported(Feature::Ip6).to_string(),
        ] {
            let hint = privilege_hint(&msg).unwrap_or_else(|| panic!("no hint for {msg:?}"));
            let lines: Vec<&str> = hint.lines().collect();
            assert_eq!(lines.len(), 2, "{hint}");
            assert!(lines[0].contains(HELPER), "{hint}");
            if cfg!(target_os = "linux") {
                assert!(lines[0].contains("setcap cap_net_raw+ep"), "{hint}");
                assert!(
                    lines[1].contains("net.ipv4.ping_group_range=\"0 2147483647\""),
                    "{hint}"
                );
            } else {
                assert!(lines[0].contains("chmod u+s"), "{hint}");
                assert!(lines[1].contains("as root"), "{hint}");
            }
            assert!(lines.iter().all(|l| l.starts_with("hint: ")), "{hint}");
        }
    }

    #[test]
    fn privilege_hint_stays_quiet_for_unrelated_failures() {
        for msg in [
            fatal_message(&ResponseKind::ProbesExhausted)
                .unwrap()
                .to_string(),
            fatal_message(&ResponseKind::AddressInUse)
                .unwrap()
                .to_string(),
            HelperError::Unsupported(Feature::Sctp).to_string(),
            HelperError::Unsupported(Feature::Mark).to_string(),
            "unexpected packet generator exit".to_string(),
            String::new(),
        ] {
            assert_eq!(privilege_hint(&msg), None, "{msg}");
        }
    }

    #[test]
    fn fatal_messages_match_cmdpipe() {
        assert_eq!(
            fatal_message(&ResponseKind::ProbesExhausted),
            Some("Probes exhausted")
        );
        assert_eq!(
            fatal_message(&ResponseKind::PermissionDenied),
            Some("mtr-rs-packet reported permission denied while sending a probe")
        );
        assert_eq!(fatal_message(&ResponseKind::NoReply), None);
    }
}
