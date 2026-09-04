// Screen snapshots of the TUI renderer on ratatui's TestBackend (spec §8, §10). GPL-2.0-only.
//
// Accept intentional changes with `INSTA_UPDATE=always cargo test -p mtr --test tui_snapshots`
// (or `cargo insta accept`) and commit the .snap files. CI runs with INSTA_UPDATE=no.

use mtr::testing::{Fixture, snapshot_fixture};
use mtr::tui::render::draw;
use mtr::tui::state::{DetailTab, Prompt, PromptKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn render(f: &Fixture, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|frame| draw(frame, &f.view())).unwrap();
    term.backend().to_string()
}

#[test]
fn unicode_80x24_rtt_tab() {
    let mut f = snapshot_fixture(false);
    f.ui.selected = 2;
    insta::assert_snapshot!("unicode_80x24_rtt", render(&f, 80, 24));
}

#[test]
fn ascii_rtt_and_addresses_tabs() {
    let mut f = snapshot_fixture(true);
    f.ui.selected = 2;
    // deviation 23: `--ascii` draws the plot itself (`*` on `|`/`-`/`+`) instead of using ratatui's
    // Chart, so the whole screen — chart included — is ASCII
    let rtt = render(&f, 80, 24);
    assert!(
        rtt.is_ascii(),
        "non-ASCII in the --ascii RTT screen:\n{rtt}"
    );
    assert!(rtt.contains('*'), "no plotted point:\n{rtt}");
    insta::assert_snapshot!("ascii_80x24_rtt", rtt);
    f.ui.selected = 0;
    f.ui.tab = DetailTab::Addresses;
    let addresses = render(&f, 80, 24);
    assert!(addresses.is_ascii(), "non-ASCII screen:\n{addresses}");
    insta::assert_snapshot!("ascii_80x24_addresses", addresses);
}

#[test]
fn addresses_and_log_tabs() {
    let mut f = snapshot_fixture(false);
    f.ui.selected = 0;
    f.ui.tab = DetailTab::Addresses;
    insta::assert_snapshot!("unicode_80x24_addresses", render(&f, 80, 24));
    f.ui.tab = DetailTab::Log;
    insta::assert_snapshot!("unicode_80x24_log", render(&f, 80, 24));
}

#[test]
fn wide_header_and_pane_hidden_when_short() {
    let mut f = snapshot_fixture(false);
    insta::assert_snapshot!("unicode_120x19_no_pane", render(&f, 120, 19));
    f.ui.pane_open = false;
    f.ui.sparkline = false;
    insta::assert_snapshot!("unicode_80x24_table_only", render(&f, 80, 24));
}

#[test]
fn help_overlay_prompt_and_too_small() {
    let mut f = snapshot_fixture(false);
    f.ui.help = true;
    insta::assert_snapshot!("unicode_80x24_help", render(&f, 80, 24));
    // spec-minimum terminal: the help box is vertically clipped, not wrapped/lost horizontally
    insta::assert_snapshot!("unicode_60x12_help", render(&f, 60, 12));
    f.ui.help = false;
    f.ui.prompt = Some(Prompt {
        kind: PromptKind::Interval,
        buf: "2.5".into(),
    });
    insta::assert_snapshot!("unicode_80x24_prompt", render(&f, 80, 24));
    insta::assert_snapshot!("too_small_50x10", render(&f, 50, 10));
}

/// The reserved `[PAUSED]` marker at the default terminal width: it and the clock both show.
#[test]
fn paused_header_at_the_default_width() {
    let mut f = snapshot_fixture(false);
    f.engine
        .handle(mtr_core::Event::Action(mtr_core::UserAction::Pause), f.now);
    let screen = render(&f, 80, 24);
    // TestBackend's Display quotes each row.
    let header = screen.lines().next().unwrap().trim_matches('"');
    assert!(header.contains("[PAUSED]"), "{header:?}");
    assert!(header.ends_with("12:34:56"), "{header:?}");
    insta::assert_snapshot!("unicode_80x24_paused", screen);
}

#[test]
fn ascii_help_overlay() {
    let mut f = snapshot_fixture(true);
    f.ui.help = true;
    let help = render(&f, 80, 24);
    assert!(help.is_ascii(), "non-ASCII screen:\n{help}");
    insta::assert_snapshot!("ascii_80x24_help", help);
}
