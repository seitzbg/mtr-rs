//! The event loop that owns the Engine — the `select_loop()` of ui/select.c on tokio. GPL-2.0-only.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use mtr_core::{Command, Engine, Event};
use mtr_proto::{Request, RequestKind};

use crate::helper::{Helper, HelperEvent, fatal_message};
use crate::names::{LookupResult, NameCache};
use crate::resolver::{LookupRequest, Resolver};

pub struct RunOutcome {
    /// Ctrl-C ended the run (exit code 130, report still printed — as C).
    pub interrupted: bool,
}

pub struct Driver<'a> {
    pub engine: &'a mut Engine,
    pub helper: &'a mut Helper,
    pub resolver: Option<&'a mut Resolver>,
    pub names: &'a mut NameCache,
}

enum Wake {
    Tick,
    Helper(Option<HelperEvent>),
    Lookup(Option<LookupResult>),
    CtrlC,
}

impl Driver<'_> {
    pub async fn run(&mut self) -> anyhow::Result<RunOutcome> {
        let mut deadline: Option<Instant> = Some(Instant::now());
        let mut ctrl_c = std::pin::pin!(tokio::signal::ctrl_c());
        loop {
            let sleep = async {
                match deadline {
                    Some(d) => tokio::time::sleep_until(d.into()).await,
                    None => std::future::pending::<()>().await,
                }
            };
            let lookups = async {
                match self.resolver.as_mut() {
                    Some(r) => r.rx.recv().await,
                    None => std::future::pending().await,
                }
            };
            let wake = tokio::select! {
                _ = sleep => Wake::Tick,
                ev = self.helper.rx.recv() => Wake::Helper(ev),
                res = lookups => Wake::Lookup(res),
                _ = &mut ctrl_c => Wake::CtrlC,
            };
            let from_engine = matches!(wake, Wake::Tick | Wake::Helper(_));
            let cmds = match wake {
                Wake::Tick => self.engine.handle(Event::Tick, Instant::now()),
                Wake::Helper(Some(HelperEvent::Response(r))) => {
                    if let Some(msg) = fatal_message(&r.kind) {
                        bail!("{msg}");
                    }
                    self.engine.handle(
                        Event::Probe {
                            token: r.token,
                            kind: r.kind,
                        },
                        Instant::now(),
                    )
                }
                Wake::Helper(Some(HelperEvent::Exited)) | Wake::Helper(None) => {
                    bail!("unexpected packet generator exit")
                }
                Wake::Lookup(res) => {
                    if let Some(r) = res {
                        self.names.apply(r);
                    }
                    Vec::new()
                }
                Wake::CtrlC => return Ok(RunOutcome { interrupted: true }),
            };
            let mut finished = false;
            for c in &cmds {
                match c.clone() {
                    Command::SendProbe { token, params } => self
                        .helper
                        .tx
                        .send(Request {
                            token,
                            kind: RequestKind::SendProbe(params),
                        })
                        .await
                        .context("mtr-packet command pipe write failure")?,
                    Command::Resolve(ip) => self.request_lookups(ip).await,
                    Command::NextWake(_) => {}
                    Command::Finished => finished = true,
                }
            }
            deadline = next_deadline(deadline, &cmds, from_engine);
            if finished {
                return Ok(RunOutcome { interrupted: false });
            }
        }
    }

    async fn request_lookups(&mut self, ip: IpAddr) {
        let cfg = self.engine.config();
        let (want_ptr, want_asn) = (cfg.dns, !cfg.ipinfo_fields.is_empty());
        let Some(res) = self.resolver.as_mut() else {
            return;
        };
        if want_ptr && self.names.request_ptr(ip) {
            let _ = res.tx.send(LookupRequest::Ptr(ip)).await;
        }
        if want_asn && self.names.request_asn(ip) {
            let _ = res.tx.send(LookupRequest::Asn(ip)).await;
        }
    }

    /// Deviation 5: give outstanding lookups up to `budget` before rendering.
    pub async fn drain_lookups(&mut self, budget: Duration) {
        let Some(res) = self.resolver.as_mut() else {
            return;
        };
        let end = tokio::time::Instant::now() + budget;
        while self.names.pending() > 0 {
            match tokio::time::timeout_at(end, res.rx.recv()).await {
                Ok(Some(r)) => self.names.apply(r),
                _ => break,
            }
        }
    }
}

/// The next wake deadline given the previous one and a processed command batch.
///
/// When `cmds` came from an engine call (`Wake::Tick` or `Wake::Helper`), the engine is
/// authoritative: a `Command::NextWake` sets the new deadline, and its absence (the engine is
/// paused) clears it, so the driver goes back to sleep indefinitely instead of busy-looping on an
/// already-elapsed deadline. When `cmds` did not come from an engine call (`Wake::Lookup`), the
/// engine was not asked and the deadline is left untouched.
fn next_deadline(current: Option<Instant>, cmds: &[Command], from_engine: bool) -> Option<Instant> {
    if !from_engine {
        return current;
    }
    cmds.iter().find_map(|c| match c {
        Command::NextWake(t) => Some(*t),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtr_proto::ProbeParams;

    #[test]
    fn engine_batch_with_next_wake_sets_deadline() {
        let now = Instant::now();
        let t = now + Duration::from_secs(1);
        let cmds = vec![Command::NextWake(t)];
        assert_eq!(next_deadline(Some(now), &cmds, true), Some(t));
    }

    #[test]
    fn engine_batch_without_next_wake_clears_deadline() {
        let now = Instant::now();
        let cmds = vec![Command::SendProbe {
            token: 1,
            params: ProbeParams::new("192.0.2.1".parse().unwrap()),
        }];
        assert_eq!(next_deadline(Some(now), &cmds, true), None);
    }

    #[test]
    fn lookup_batch_leaves_deadline_unchanged() {
        let now = Instant::now();
        let cmds: Vec<Command> = Vec::new();
        assert_eq!(next_deadline(Some(now), &cmds, false), Some(now));
        assert_eq!(next_deadline(None, &cmds, false), None);
    }
}
