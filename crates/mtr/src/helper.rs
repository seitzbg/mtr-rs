//! The `mtr-packet` child: search order and spawn (ui/cmdpipe.c:240-372), startup handshake
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

#[derive(Debug, Error)]
pub enum HelperError {
    #[error("mtr-packet not found; tried: {}", .0.join(", "))]
    NotFound(Vec<String>),
    #[error(
        "mtr-packet did not answer the startup check; check that it is installed and allowed to open probe sockets"
    )]
    StartupCheck,
    #[error("Packet type unsupported: {}", .0.as_str())]
    Unsupported(Feature),
    #[error("mtr-packet command pipe failure: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug)]
pub enum HelperEvent {
    Response(Response),
    /// The helper's stdout closed: it exited. Fatal, as in C ("unexpected packet generator exit").
    Exited,
}

/// A running helper. Dropping it kills the child (`kill_on_drop`).
pub struct Helper {
    pub tx: mpsc::Sender<Request>,
    pub rx: mpsc::Receiver<HelperEvent>,
    _child: Child,
}

/// `execute_packet_child()`: `$MTR_PACKET` (ignored when `/etc/mtr.is.run.under.sudo` exists),
/// `mtr-packet` on `PATH`, next to our own executable, `./mtr-packet`.
pub fn candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if !Path::new("/etc/mtr.is.run.under.sudo").exists() {
        if let Some(p) = std::env::var_os("MTR_PACKET") {
            v.push(PathBuf::from(p));
        }
    }
    v.push(PathBuf::from("mtr-packet"));
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        v.push(dir.join("mtr-packet"));
    }
    v.push(PathBuf::from("./mtr-packet"));
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
                tracing::error!("mtr-packet command pipe write failure: {e}");
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
                    Err(e) => tracing::warn!("unparsable reply from mtr-packet {line:?}: {e}"),
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

/// The replies `handle_reply_errors()` (cmdpipe.c:690-728) treats as fatal, with its messages.
pub fn fatal_message(kind: &ResponseKind) -> Option<&'static str> {
    Some(match kind {
        ResponseKind::ProbesExhausted => "Probes exhausted",
        ResponseKind::InvalidArgument { .. } => "mtr-packet reported invalid argument",
        ResponseKind::PermissionDenied => {
            "mtr-packet reported permission denied while sending a probe"
        }
        ResponseKind::AddressInUse => "Address in use",
        ResponseKind::AddressNotAvailable => "Address not available",
        ResponseKind::UnexpectedError { .. } => "Unexpected mtr-packet error",
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

    #[tokio::test]
    async fn missing_helper_lists_the_candidates() {
        let err = spawn_with(
            &[PathBuf::from("/nonexistent/mtr-packet")],
            false,
            Protocol::Icmp,
            0,
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(err, HelperError::NotFound(_)));
        assert!(err.to_string().contains("/nonexistent/mtr-packet"));
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
        assert_eq!(c.last().unwrap(), &PathBuf::from("./mtr-packet"));
        assert!(c.iter().any(|p| p == &PathBuf::from("mtr-packet")));
    }

    #[test]
    fn fatal_messages_match_cmdpipe() {
        assert_eq!(
            fatal_message(&ResponseKind::ProbesExhausted),
            Some("Probes exhausted")
        );
        assert_eq!(
            fatal_message(&ResponseKind::PermissionDenied),
            Some("mtr-packet reported permission denied while sending a probe")
        );
        assert_eq!(fatal_message(&ResponseKind::NoReply), None);
    }
}
