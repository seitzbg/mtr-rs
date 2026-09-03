// Drives the real interactive loop on a TestBackend with the fake helper and scripted keys. GPL-2.0-only.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use futures::channel::mpsc;
use mtr::driver::Driver;
use mtr::emit::{ReportContext, report_on_exit_text};
use mtr::helper::spawn_with;
use mtr::names::NameCache;
use mtr::tui::{Depth, Glyphs, Palette, TuiOptions, run};
use mtr_core::{Config, Engine};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn fake() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fake-mtr-packet.py"
    ))
}

fn key(c: char) -> std::io::Result<Event> {
    Ok(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )))
}

fn ctrl(c: char) -> std::io::Result<Event> {
    Ok(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::CONTROL,
    )))
}

struct Session {
    interrupted: bool,
    engine: Engine,
    names: NameCache,
    screen: String,
}

/// `keys` are `(delay_ms_after_the_previous_key, event)` — the sender sleeps between sends, so the
/// delays are *relative*, and a key's wall-clock time is the running sum of the delays before it.
async fn session(keys: Vec<(u64, std::io::Result<Event>)>, cfg: Config) -> Session {
    let mut helper = spawn_with(&[fake()], false, mtr_proto::Protocol::Icmp, 0)
        .await
        .unwrap();
    let mut engine = Engine::new(cfg, "192.0.2.1".parse().unwrap(), None, Instant::now(), 1);
    let mut names = NameCache::default();
    let (tx, rx) = mpsc::unbounded();
    tokio::spawn(async move {
        for (delay_ms, ev) in keys {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            if tx.unbounded_send(ev).is_err() {
                break;
            }
        }
    });
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let interrupted = {
        let mut driver = Driver::new(&mut engine, &mut helper, None, &mut names);
        let opts = TuiOptions {
            glyphs: Glyphs::select(false),
            palette: Palette::new(Depth::Mono),
            is_root: true,
            local_hostname: "testhost",
            target_name: "192.0.2.1",
        };
        let out = tokio::time::timeout(
            Duration::from_secs(20),
            run(&mut term, &mut driver, rx, &opts),
        )
        .await
        .expect("loop ended within 20 s")
        .unwrap();
        out.interrupted
    };
    Session {
        interrupted,
        engine,
        names,
        screen: term.backend().to_string(),
    }
}

#[tokio::test]
async fn pause_reset_and_quit_drive_the_engine() {
    // relative delays; absolute times are 0.7 s `p`, 1.9 s space, 3.4 s `r`, 5.3 s `q`
    // (interval 1 s / numhosts 10 → 100 ms ticks over a 3-hop path)
    let keys = vec![
        (700, key('p')),
        (1200, key(' ')),
        (1500, key('r')),
        (1900, key('q')),
    ];
    let s = session(
        keys,
        Config {
            interval: 1.0,
            ..Config::default()
        },
    )
    .await;
    assert!(!s.interrupted);
    assert!(!s.engine.paused(), "resumed by space");
    // reset at 3.4 s, quit at 5.3 s: about two 1 s cycles of a 3-hop path after the reset, with
    // room for a scheduler hiccup on either side
    let sent: i32 = s.engine.hops().iter().map(|h| h.transmitted()).sum();
    assert!((1..=12).contains(&sent), "sent after reset: {sent}");
    assert!(
        s.screen.contains("192.0.2.1"),
        "target row rendered:\n{}",
        s.screen
    );
}

#[tokio::test]
async fn ctrl_c_is_interrupted_and_pause_freezes_probing() {
    // relative delays: `p` at 0.3 s, Ctrl-C at 1.6 s
    let keys = vec![(300, key('p')), (1300, ctrl('c'))];
    let s = session(
        keys,
        Config {
            interval: 1.0,
            ..Config::default()
        },
    )
    .await;
    assert!(s.interrupted);
    assert!(s.engine.paused());
    let sent: i32 = s.engine.hops().iter().map(|h| h.transmitted()).sum();
    assert!(sent <= 6, "no probes while paused: {sent}");
    assert!(s.screen.contains("[PAUSED]"), "{}", s.screen);
}

#[tokio::test]
async fn cycles_finish_the_loop_and_leave_a_printable_exit_report() {
    let cfg = Config {
        max_ping: 1,
        force_max_ping: true,
        grace_time: 0.2,
        ..Config::default()
    };
    // one never-arriving key keeps the sender alive: dropping it early would end the loop by EOF
    // (the `stdin_eof_quits` path) instead of by `-c`
    let s = session(vec![(5_000, key('x'))], cfg).await;
    assert!(!s.interrupted);
    assert!(s.engine.is_finished());
    // Task 15's exit path, without a process or a TTY
    let ctx = ReportContext {
        engine: &s.engine,
        names: &s.names,
        local_hostname: "testhost",
        target_name: "192.0.2.1",
        wide: false,
        fields: mtr_core::fields::active_fields(&s.engine.config().fields),
    };
    let report = report_on_exit_text(&ctx, true);
    assert!(report.starts_with("HOST: "), "{report}");
    assert!(report.contains("  3.|-- 192.0.2.1"), "{report}");
    assert!(!report.contains("Start: "), "report_close() only: {report}");
    assert!(report_on_exit_text(&ctx, false).is_empty());
}

#[tokio::test]
async fn stdin_eof_quits() {
    // empty `keys` → the sender task ends at once → the stream yields None → quit
    let s = session(vec![], Config::default()).await;
    assert!(!s.interrupted);
    // `Config::default()` has force_max_ping false, so the loop cannot have ended by itself:
    // returning at all proves the EOF quit fired
    assert!(!s.engine.is_finished(), "quit came from EOF, not from -c");
}
