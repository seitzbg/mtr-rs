//! mtr client library: CLI, helper process, resolver, engine driver and emitters.
//! Rust port of the `ui/` half of mtr 0.96 (commit 7b01773). GPL-2.0-only.
#![forbid(unsafe_code)]

pub mod asn;
pub mod cli;
pub mod driver;
pub mod emit;
pub mod helper;
pub mod names;
pub mod options;
pub mod resolver;
pub mod target;
pub mod tui;
pub mod width;

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mtr_core::Engine;

use crate::cli::{AddressFamily, Args, Options, OutputMode, Target};
use crate::driver::Driver;
use crate::names::NameCache;
use crate::resolver::{Resolver, ResolverConfig};

/// `MTR_RS_LOG=<file>` enables tracing output (level via `MTR_RS_LOG_LEVEL`, default `debug`).
fn init_logging() {
    let Some(path) = std::env::var_os("MTR_RS_LOG") else {
        return;
    };
    let Ok(file) = std::fs::File::create(&path) else {
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
    init_logging();
    let env_options = std::env::var("MTR_OPTIONS").ok();
    match cli::build_argv(env_options.as_deref(), std::env::args().skip(1)) {
        Ok(argv) => run(argv).await,
        Err(msg) => {
            eprintln!("mtr: {msg}");
            1
        }
    }
}

enum Fatal {
    /// Skip this target, continue with the next one, exit 1 at the end (C: resolution failures).
    Skip(String),
    /// Stop immediately with exit 1 (C: `error(EXIT_FAILURE, …)`).
    Abort(String),
}

enum TargetOutcome {
    Done,
    Interrupted,
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
    if let Some(file) = args.filename.take() {
        match options::hosts_from_file_option(&file, helper::sudo_guard_present()) {
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
    if opts.mode == OutputMode::Tui {
        eprintln!("mtr: interactive mode is not implemented yet; use -r, -w, -j or -C");
        return 1;
    }

    // validate_report_targets() (ui/mtr.c:1089-1131): the first target's family becomes the
    // getaddrinfo() hint for every later target, so a dual-stack host follows the first one and
    // only a host with no address in that family fails (C: EAI_ADDRFAMILY).
    let mut af = opts.af;
    if opts.targets.len() > 1 {
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
    for t in &opts.targets {
        match run_target(&opts, t, af).await {
            Ok(TargetOutcome::Done) => {}
            Ok(TargetOutcome::Interrupted) => {
                exit_val = 130;
                break;
            }
            Err(Fatal::Skip(msg)) => {
                eprintln!("mtr: {msg}");
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
    exit_val
}

async fn run_target(opts: &Options, t: &Target, af: AddressFamily) -> Result<TargetOutcome, Fatal> {
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
        let outcome = driver
            .run()
            .await
            .map_err(|e| Fatal::Abort(e.to_string()))?;
        if outcome.interrupted {
            driver.engine.end_transit(); // net_end_transit() before display_close()
        }
        driver.drain_lookups(Duration::from_secs(2)).await;
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
    match opts.mode {
        OutputMode::Report => print!("{}", emit::report::render(&ctx)),
        OutputMode::Json => print!("{}", emit::json::render(&ctx)),
        OutputMode::Csv => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            print!("{}", emit::csv::render(&ctx, now));
        }
        OutputMode::Tui => unreachable!("rejected before probing"),
    }
    Ok(if interrupted {
        TargetOutcome::Interrupted
    } else {
        TargetOutcome::Done
    })
}
