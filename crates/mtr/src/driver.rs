//! The event loop that owns the Engine — the `select_loop()` of ui/select.c on tokio. GPL-2.0-only.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use mtr_core::{Command, Engine, Event, UserAction};
use mtr_proto::{Request, RequestKind};

use crate::helper::{Helper, HelperEvent, fatal_message};
use crate::names::{LookupResult, NameCache};
use crate::resolver::{LookupRequest, Resolver};

pub struct RunOutcome {
    /// Ctrl-C ended the run (exit code 130, report still printed — as C).
    pub interrupted: bool,
}

/// The next thing for `step()` to react to.
pub enum Wake {
    Tick,
    Helper(Option<HelperEvent>),
    Lookup(Option<LookupResult>),
    /// A key the TUI translated into an engine action.
    Action(UserAction),
}

/// The outcome of one `step()` call.
#[derive(Debug)]
pub struct Step {
    /// The engine emitted `Command::Finished`.
    pub finished: bool,
}

pub struct Driver<'a> {
    pub engine: &'a mut Engine,
    pub helper: &'a mut Helper,
    pub resolver: Option<&'a mut Resolver>,
    pub names: &'a mut NameCache,
    /// Next `Wake::Tick`; `None` while the engine is paused or finished.
    pub deadline: Option<Instant>,
}

impl<'a> Driver<'a> {
    pub fn new(
        engine: &'a mut Engine,
        helper: &'a mut Helper,
        resolver: Option<&'a mut Resolver>,
        names: &'a mut NameCache,
    ) -> Self {
        Driver {
            engine,
            helper,
            resolver,
            names,
            deadline: Some(Instant::now()),
        }
    }

    /// The next thing to react to: a due tick, a helper line, or a lookup result.
    pub async fn wait_wake(&mut self) -> Wake {
        let deadline = self.deadline;
        let sleep = async move {
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
        tokio::select! {
            _ = sleep => Wake::Tick,
            ev = self.helper.rx.recv() => Wake::Helper(ev),
            res = lookups => Wake::Lookup(res),
        }
    }

    /// Apply one wake to the engine, dispatch its commands, update the deadline.
    pub async fn step(&mut self, wake: Wake) -> anyhow::Result<Step> {
        let now = Instant::now();
        let from_engine = !matches!(wake, Wake::Lookup(_));
        let mut relookup = false;
        let cmds = match wake {
            Wake::Tick => self.engine.handle(Event::Tick, now),
            Wake::Helper(Some(HelperEvent::Response(r))) => {
                if let Some(msg) = fatal_message(&r.kind) {
                    bail!("{msg}");
                }
                self.engine.handle(
                    Event::Probe {
                        token: r.token,
                        kind: r.kind,
                    },
                    now,
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
            Wake::Action(a) => {
                let before = (
                    self.engine.config().dns,
                    self.engine.config().ipinfo_fields.is_empty(),
                );
                let cmds = self.engine.handle(Event::Action(a), now);
                let after = (
                    self.engine.config().dns,
                    self.engine.config().ipinfo_fields.is_empty(),
                );
                relookup = (!before.0 && after.0) || (before.1 && !after.1);
                cmds
            }
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
                    .context("mtr-rs-packet command pipe write failure")?,
                Command::Resolve(ip) => self.request_lookups(ip).await,
                Command::NextWake(_) => {}
                Command::Finished => finished = true,
            }
        }
        if relookup {
            // Deviation 19: names/ASNs for addresses discovered while the lookup kind was off.
            let known: Vec<IpAddr> = self
                .engine
                .hops()
                .iter()
                .flat_map(|h| h.addrs.iter().map(|a| a.addr))
                .collect();
            for ip in known {
                self.request_lookups(ip).await;
            }
        }
        self.deadline = next_deadline(self.deadline, &cmds, from_engine);
        Ok(Step { finished })
    }

    pub async fn run(&mut self) -> anyhow::Result<RunOutcome> {
        let mut ctrl_c = std::pin::pin!(tokio::signal::ctrl_c());
        loop {
            let wake = tokio::select! {
                w = self.wait_wake() => w,
                _ = &mut ctrl_c => return Ok(RunOutcome { interrupted: true }),
            };
            if self.step(wake).await?.finished {
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

    fn fake() -> std::path::PathBuf {
        std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fake-mtr-packet.py"
        ))
    }

    #[tokio::test]
    async fn step_drives_one_wake_and_tracks_the_deadline() {
        use crate::helper::spawn_with;
        use mtr_core::{Config, Engine, UserAction};
        let mut helper = spawn_with(&[fake()], false, mtr_proto::Protocol::Icmp, 0)
            .await
            .unwrap();
        let cfg = Config {
            interactive: false,
            max_ttl: 3,
            max_ping: 1,
            grace_time: 0.1,
            ..Config::default()
        };
        let mut engine = Engine::new(cfg, "192.0.2.1".parse().unwrap(), None, Instant::now(), 1);
        let mut names = NameCache::default();
        let mut d = Driver::new(&mut engine, &mut helper, None, &mut names);
        assert!(d.deadline.is_some());
        let s = d.step(Wake::Tick).await.unwrap();
        assert!(!s.finished);
        assert_eq!(d.engine.hops()[0].stats.xmit, 1);
        // the fake answers at once
        let w = tokio::time::timeout(Duration::from_secs(2), d.wait_wake())
            .await
            .unwrap();
        assert!(matches!(w, Wake::Helper(Some(HelperEvent::Response(_)))));
        d.step(w).await.unwrap();
        assert_eq!(d.engine.hops()[0].stats.returned, 1);
        // pausing clears the deadline; resuming restores it
        d.step(Wake::Action(UserAction::Pause)).await.unwrap();
        assert_eq!(d.deadline, None);
        d.step(Wake::Action(UserAction::Resume)).await.unwrap();
        assert!(d.deadline.is_some());
        // a lookup wake does not touch the deadline
        let before = d.deadline;
        d.step(Wake::Lookup(None)).await.unwrap();
        assert_eq!(d.deadline, before);
        // run to completion: 3 probes, 1 cycle, 0.1 s grace
        let out = tokio::time::timeout(Duration::from_secs(5), d.run())
            .await
            .unwrap()
            .unwrap();
        assert!(!out.interrupted);
        assert!(d.engine.is_finished());
    }

    #[tokio::test]
    async fn helper_exit_is_a_step_error() {
        use crate::helper::spawn_with;
        use mtr_core::{Config, Engine};
        let mut helper = spawn_with(&[fake()], false, mtr_proto::Protocol::Icmp, 0)
            .await
            .unwrap();
        let mut engine = Engine::new(
            Config::default(),
            "192.0.2.1".parse().unwrap(),
            None,
            Instant::now(),
            1,
        );
        let mut names = NameCache::default();
        let mut d = Driver::new(&mut engine, &mut helper, None, &mut names);
        let err = d
            .step(Wake::Helper(Some(HelperEvent::Exited)))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "unexpected packet generator exit");
    }

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
