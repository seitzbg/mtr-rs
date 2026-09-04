//! Single-line header (spec §8 item 1); replaces the two title lines of ui/curses.c:905-935.
//! GPL-2.0-only.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use crate::tui::protocol_name;
use crate::tui::render::View;
use crate::width::display_width;

pub fn render(view: &View, area: Rect, buf: &mut Buffer) {
    let e = view.engine;
    let cfg = e.config();
    let p = view.palette;
    let mut spans = vec![Span::styled(
        format!("{} {}  ", crate::cli::PROGRAM, view.version),
        p.header(),
    )];
    if area.width >= 100 {
        let local = e
            .local()
            .map(|a| a.to_string())
            .unwrap_or_else(|| "?".into());
        spans.push(Span::raw(format!("{} ({local})  ", view.local_hostname)));
    }
    spans.push(Span::styled(format!("{}  ", view.glyphs.arrow), p.accent()));
    spans.push(Span::styled(
        format!("{} ({})", view.target_name, e.target()),
        p.bold(),
    ));
    let port = if cfg.remote_port != 0 {
        format!(":{}", cfg.remote_port)
    } else {
        String::new()
    };
    spans.push(Span::raw(format!(
        "   {}/IPv{}{port}  i={}s  Snt {}",
        protocol_name(cfg.protocol),
        if e.target().is_ipv6() { 6 } else { 4 },
        cfg.interval,
        e.cycles_done()
    )));
    if e.paused() {
        spans.push(Span::styled("  [PAUSED]", p.alert()));
    }
    // The clock is written last but its cells are reserved first: the line left of it is limited to
    // `width - clock_w - 1` display columns (`set_line`'s max_width is a display width), so a long
    // header drops its tail instead of being overwritten by the clock.
    let clock_w = display_width(view.clock) as u16;
    let left_w = area.width.saturating_sub(clock_w + 1);
    buf.set_line(area.x, area.y, &Line::from(spans), left_w);
    if area.width > clock_w {
        buf.set_string(area.x + area.width - clock_w, area.y, view.clock, p.dim());
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{Fixture, row_text, view_fixture};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    /// The header line rendered `width` cells wide.
    fn line(f: &Fixture, width: u16) -> String {
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        super::render(&f.view(), area, &mut buf);
        row_text(&buf, 0)
    }

    #[test]
    fn header_shows_target_protocol_interval_cycles_and_clock() {
        let f = view_fixture();
        let l = line(&f, 80);
        assert!(
            l.starts_with(&format!(
                "mtr-rs {}  →  target.example (192.0.2.10)   ICMP/IPv4  i=1s  Snt 2",
                f.version
            )),
            "{l:?}"
        );
        assert!(l.ends_with("12:34:56"), "{l:?}");
        assert!(
            !l.contains("testhost"),
            "local host only at >= 100 columns: {l:?}"
        );
        let l = line(&f, 120);
        assert!(
            l.contains("testhost (192.0.2.1)  →  target.example"),
            "{l:?}"
        );
        assert!(l.ends_with("12:34:56"), "{l:?}");
    }

    #[test]
    fn the_clock_column_is_reserved_and_the_rest_truncates() {
        let mut f = view_fixture();
        f.engine
            .handle(mtr_core::Event::Action(mtr_core::UserAction::Pause), f.now);
        // 120 columns: 22 (local host) + 66 (mtr-rs … Snt 2) + 10 ("  [PAUSED]") = 98 <= 120 - 9
        let l = line(&f, 120);
        assert!(l.contains("[PAUSED]"), "{l:?}");
        assert!(l.ends_with("12:34:56"), "{l:?}");
        // 100 columns: the same 98 cells do not fit in 100 - 8 - 1 = 91, so the tail is cut …
        let l = line(&f, 100);
        assert!(
            !l.contains("[PAUSED]"),
            "left segment truncated to width - 9: {l:?}"
        );
        // … never at the clock's expense, and one blank cell stays between the two
        assert!(l.ends_with("12:34:56"), "{l:?}");
        assert_eq!(l.chars().nth(91), Some(' '), "{l:?}");
        // 80 columns: only the first 71 cells of the left segment survive
        let l = line(&f, 80);
        assert!(l.starts_with("mtr-rs "), "{l:?}");
        assert!(!l.contains("[PAUSED]"), "{l:?}");
        assert!(l.ends_with("12:34:56"), "{l:?}");
    }
}
