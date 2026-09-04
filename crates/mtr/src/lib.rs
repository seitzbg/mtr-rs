//! mtr client library: CLI, helper process, resolver, engine driver and emitters.
//! Rust port of the `ui/` half of mtr 0.96 (commit 7b01773). GPL-2.0-only.
#![forbid(unsafe_code)]

pub mod asn;
pub mod cli;
pub mod config_file;
pub mod driver;
pub mod emit;
pub mod helper;
pub mod names;
pub mod options;
pub mod resolver;
pub mod target;
#[doc(hidden)]
pub mod testing;
pub mod tui;
pub mod width;

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mtr_core::Engine;

use crate::cli::{AddressFamily, Args, Options, OutputMode, Target};
use crate::driver::Driver;
use crate::names::NameCache;
use crate::resolver::{Resolver, ResolverConfig};

/// `MTR_RS_LOG=<file>` enables tracing output (level via `MTR_RS_LOG_LEVEL`, default `debug`).
/// Ignored under the sudo guard: the path comes from the environment of a possibly privileged
/// invocation, the same rule as `$MTR_PACKET`, `-F` and `--config`.
pub fn init_logging(sudo_guard: bool) {
    let Some(path) = std::env::var_os("MTR_RS_LOG") else {
        return;
    };
    init_logging_to(sudo_guard, std::path::Path::new(&path));
}

fn init_logging_to(sudo_guard: bool, path: &std::path::Path) {
    if sudo_guard {
        return;
    }
    // create_new: never truncate or follow an existing file into somewhere we did not intend.
    let Ok(file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    else {
        return;
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("MTR_RS_LOG_LEVEL")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("debug"));
    let _ = tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .with_env_filter(filter)
        .try_init();
}

/// Entry point used by the binary: environment + argv → exit code.
pub async fn run_from_env() -> i32 {
    let env_options = std::env::var("MTR_OPTIONS").ok();
    match cli::build_argv(env_options.as_deref(), std::env::args().skip(1)) {
        Ok(argv) => run(argv).await,
        Err(msg) => {
            eprintln!("mtr: {msg}");
            1
        }
    }
}

/// The TUI panic hook is process-wide; installing it per target would nest hooks.
static PANIC_HOOK: std::sync::Once = std::sync::Once::new();

enum Fatal {
    /// Skip this target, continue with the next one, exit 1 at the end (C: resolution failures).
    Skip(String),
    /// Stop immediately with exit 1 (C: `error(EXIT_FAILURE, …)`).
    Abort(String),
}

/// The `Option<String>` carries the rendered JSON document (`Some` in JSON mode only); the other
/// modes print as they go, because their output is a stream whose order matters.
enum TargetOutcome {
    Done(Option<String>),
    Interrupted(Option<String>),
}

/// ui/mtr.c:1272-1273: the interactive display runs the first target and stops.
fn targets_to_run(mode: OutputMode, targets: &[Target]) -> &[Target] {
    if mode == OutputMode::Tui {
        &targets[..targets.len().min(1)]
    } else {
        targets
    }
}

/// ui/mtr.c:1238-1245: a target that does not resolve ends an interactive run; the report
/// modes skip it and carry on with exit status 1.
fn resolve_failure_is_fatal(mode: OutputMode) -> bool {
    mode == OutputMode::Tui
}

/// Parse, validate and run every target; returns the process exit code.
pub async fn run(argv: Vec<String>) -> i32 {
    let mut args = match Args::parse_argv(argv) {
        Ok(a) => a,
        Err(e) => {
            let code = if e.use_stderr() { 1 } else { 0 };
            let _ = e.print();
            return code;
        }
    };
    if args.version > 0 {
        print!("{}", cli::version_text(args.version));
        return 0;
    }
    let sudo_guard = helper::sudo_guard_present();
    init_logging(sudo_guard);
    if args.init_config {
        let p = match config_file::init_config_target(args.config.as_deref(), sudo_guard) {
            Ok(p) => p,
            Err(msg) => {
                eprintln!("mtr: config: {msg}");
                return 1;
            }
        };
        return match config_file::init(&p) {
            Ok(()) => {
                println!("{}", p.display());
                0
            }
            Err(msg) => {
                eprintln!("mtr: config: {msg}");
                1
            }
        };
    }
    let cfg_path = match config_file::config_source(args.config.as_deref(), sudo_guard) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("mtr: {msg}");
            return 1;
        }
    };
    // A `None` path means no `$HOME` and no absolute `$XDG_CONFIG_HOME`, i.e. there is no file to
    // read — the same situation as a file that does not exist.
    if let Some(p) = &cfg_path {
        match config_file::load(p) {
            Ok(cfg) => config_file::apply(&mut args, &cfg),
            Err(msg) => {
                eprintln!("mtr: config: {msg}");
                return 1;
            }
        }
    }
    if let Some(file) = args.filename.take() {
        match options::hosts_from_file_option(&file, sudo_guard) {
            Ok(mut names) => {
                names.append(&mut args.hosts);
                args.hosts = names;
            }
            Err(msg) => {
                eprintln!("mtr: {msg}");
                return 1;
            }
        }
    }
    let is_root = nix::unistd::Uid::current().is_root();
    let opts = match args.into_options(is_root) {
        Ok(o) => o,
        Err(msg) => {
            eprintln!("mtr: {msg}");
            return 1;
        }
    };

    // validate_report_targets() (ui/mtr.c:1089-1131): the first target's family becomes the
    // getaddrinfo() hint for every later target, so a dual-stack host follows the first one and
    // only a host with no address in that family fails (C: EAI_ADDRFAMILY).
    let mut af = opts.af;
    if opts.mode != OutputMode::Tui && opts.targets.len() > 1 {
        let requested = opts.af;
        for t in &opts.targets {
            match target::resolve_target(&t.name, af).await {
                Ok(ip) => {
                    af = if ip.is_ipv6() {
                        AddressFamily::V6
                    } else {
                        AddressFamily::V4
                    }
                }
                Err(msg) => {
                    if af != requested && target::resolve_target(&t.name, requested).await.is_ok() {
                        eprintln!("mtr: multiple report targets must use the same address family");
                    } else {
                        eprintln!("mtr: {msg}");
                    }
                    return 1;
                }
            }
        }
    }

    let mut exit_val = 0;
    let mut json_docs: Vec<String> = Vec::new();
    for t in targets_to_run(opts.mode, &opts.targets) {
        match run_target(&opts, t, af, is_root).await {
            Ok(TargetOutcome::Done(doc)) => json_docs.extend(doc),
            Ok(TargetOutcome::Interrupted(doc)) => {
                // C prints the current target's JSON on SIGINT too.
                json_docs.extend(doc);
                exit_val = 130;
                break;
            }
            Err(Fatal::Skip(msg)) => {
                eprintln!("mtr: {msg}");
                if resolve_failure_is_fatal(opts.mode) {
                    return 1;
                }
                exit_val = 1;
            }
            Err(Fatal::Abort(msg)) => {
                eprintln!("mtr: {msg}");
                if msg == helper::fatal_message(&mtr_proto::ResponseKind::PermissionDenied).unwrap()
                {
                    eprintln!("mtr: hint: sudo setcap cap_net_raw+ep \"$(command -v mtr-packet)\"");
                }
                return 1;
            }
        }
    }
    if !json_docs.is_empty() {
        print!("{}", emit::json::wrap_documents(&json_docs));
    }
    exit_val
}

async fn run_target(
    opts: &Options,
    t: &Target,
    af: AddressFamily,
    is_root: bool,
) -> Result<TargetOutcome, Fatal> {
    let ip = target::resolve_target(&t.name, af)
        .await
        .map_err(Fatal::Skip)?;
    let mut cfg = opts.config.clone();
    if t.port != 0 {
        cfg.remote_port = t.port;
    }
    let local = match (opts.source_address, &cfg.interface) {
        (Some(a), _) => Some(target::validate_source_address(a, ip).map_err(Fatal::Abort)?),
        (None, Some(ifname)) => {
            Some(target::interface_address(ifname, ip.is_ipv6()).map_err(Fatal::Abort)?)
        }
        (None, None) => target::find_local_address(ip, cfg.mark).map_err(Fatal::Abort)?,
    };
    let local_hostname = target::local_hostname();
    let mut helper = helper::spawn(ip.is_ipv6(), cfg.protocol, cfg.mark)
        .await
        .map_err(|e| Fatal::Abort(e.to_string()))?;
    if opts.mode == OutputMode::Report {
        println!("{}", emit::report::start_line(&jiff::Zoned::now()));
    }
    let mut resolver = if cfg.dns || !cfg.ipinfo_fields.is_empty() {
        Some(
            Resolver::start(ResolverConfig {
                provider4: opts.ipinfo_provider4.clone(),
                provider6: opts.ipinfo_provider6.clone(),
            })
            .map_err(Fatal::Abort)?,
        )
    } else {
        None
    };
    let mut names = NameCache::default();
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    let mut engine = Engine::new(cfg, ip, local, Instant::now(), seed);
    let interrupted = {
        let mut driver = Driver::new(&mut engine, &mut helper, resolver.as_mut(), &mut names);
        let outcome = if opts.mode == OutputMode::Tui {
            let guard =
                tui::terminal::enter().map_err(|e| Fatal::Abort(format!("terminal: {e}")))?;
            // Only once, and only with a live Guard: the hook restores the terminal, which is a
            // no-op at best and stray escape bytes at worst when no TUI is running.
            PANIC_HOOK.call_once(tui::terminal::install_panic_hook);
            let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
            let mut term = ratatui::Terminal::new(backend)
                .map_err(|e| Fatal::Abort(format!("terminal: {e}")))?;
            let tui_opts = tui::TuiOptions {
                glyphs: tui::Glyphs::select(opts.ascii),
                sparkline: opts.sparkline,
                detail_pane: opts.detail_pane,
                palette: tui::Palette::detect(opts.color).with_rtt_thresholds(opts.rtt_thresholds),
                is_root,
                local_hostname: &local_hostname,
                target_name: &t.name,
            };
            let r = tui::run(
                &mut term,
                &mut driver,
                crossterm::event::EventStream::new(),
                &tui_opts,
            )
            .await;
            drop(guard); // restore before any message is printed
            // `{:#}` keeps anyhow's context chain ("terminal input: <cause>").
            let r = r.map_err(|e| Fatal::Abort(format!("{e:#}")))?;
            driver::RunOutcome {
                interrupted: r.interrupted,
            }
        } else {
            driver
                .run()
                .await
                .map_err(|e| Fatal::Abort(e.to_string()))?
        };
        if outcome.interrupted || opts.mode == OutputMode::Tui {
            // display.c:145-152 calls net_end_transit() on every close path, not just Ctrl-C, so
            // `q --report-on-exit` reports in-flight probes as drops too (spec §8.3).
            driver.engine.end_transit();
        }
        // Only wait for in-flight PTR/ASN lookups when something below will print them: the TUI
        // without --report-on-exit prints nothing, and `q` should not stall on a blank terminal.
        if opts.mode != OutputMode::Tui || opts.report_on_exit {
            driver.drain_lookups(Duration::from_secs(2)).await;
        }
        outcome.interrupted
    };
    let ctx = emit::ReportContext {
        engine: &engine,
        names: &names,
        local_hostname: &local_hostname,
        target_name: &t.name,
        wide: opts.report_wide,
        fields: mtr_core::fields::active_fields(&engine.config().fields),
    };
    let mut json_doc = None;
    match opts.mode {
        OutputMode::Report => print!("{}", emit::report::render(&ctx)),
        OutputMode::Json => json_doc = Some(emit::json::render(&ctx)),
        OutputMode::Csv => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            print!("{}", emit::csv::render(&ctx, now));
        }
        OutputMode::Tui => print!("{}", emit::report_on_exit_text(&ctx, opts.report_on_exit)),
    }
    Ok(if interrupted {
        TargetOutcome::Interrupted(json_doc)
    } else {
        TargetOutcome::Done(json_doc)
    })
}

#[cfg(test)]
mod tests {
    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mtr-rs-log-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("mtr.log")
    }

    #[test]
    fn logging_is_disabled_under_the_sudo_guard() {
        let path = temp("guard");
        super::init_logging_to(true, &path);
        assert!(
            !path.exists(),
            "log file must not be created under the sudo guard"
        );
    }

    #[test]
    fn interactive_mode_runs_only_the_first_target() {
        use crate::cli::{OutputMode, Target};
        let t = |n: &str| Target {
            name: n.to_string(),
            port: 0,
        };
        let all = vec![t("a"), t("b")];
        assert_eq!(super::targets_to_run(OutputMode::Tui, &all).len(), 1);
        assert_eq!(super::targets_to_run(OutputMode::Report, &all).len(), 2);
        assert_eq!(super::targets_to_run(OutputMode::Json, &all).len(), 2);
        assert!(super::resolve_failure_is_fatal(OutputMode::Tui));
        assert!(!super::resolve_failure_is_fatal(OutputMode::Csv));
    }

    #[test]
    fn logging_never_truncates_an_existing_file() {
        let path = temp("existing");
        std::fs::write(&path, "keep me\n").unwrap();
        super::init_logging_to(false, &path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep me\n");
    }
}
