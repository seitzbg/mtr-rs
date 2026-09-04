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
    // The clock and, while probing is paused, the `[PAUSED]` marker are written last but their
    // cells are reserved first: the line left of them is limited to what remains (`set_line`'s
    // max_width is a display width), so a long header drops its tail instead of being overwritten
    // by the clock or silently losing the pause marker. Both are right-aligned, the marker
    // immediately left of the clock, each with one blank column in front of it.
    let clock_w = display_width(view.clock) as u16;
    let marker_w = if e.paused() {
        display_width(PAUSED) as u16 + 1
    } else {
        0
    };
    let left_w = area.width.saturating_sub(clock_w + 1 + marker_w);
    buf.set_line(area.x, area.y, &Line::from(spans), left_w);
    if marker_w != 0 && area.width > clock_w + marker_w {
        buf.set_string(
            area.x + area.width - clock_w - marker_w,
            area.y,
            PAUSED,
            p.alert(),
        );
    }
    if area.width > clock_w {
        buf.set_string(area.x + area.width - clock_w, area.y, view.clock, p.dim());
    }
}

/// The pause marker, reserved next to the clock so it survives a long header.
const PAUSED: &str = "[PAUSED]";

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
    fn the_clock_and_pause_marker_columns_are_reserved_and_the_rest_truncates() {
        let mut f = view_fixture();
        f.engine
            .handle(mtr_core::Event::Action(mtr_core::UserAction::Pause), f.now);
        // The left segment is 69 cells ("mtr-rs 0.1.0  " 14 + "→  " 3 + "target.example
        // (192.0.2.10)" 27 + "   ICMP/IPv4  i=1s  Snt 2" 25), plus 22 more for the local host
        // once the line is at least 100 wide. The clock reserves 8 + 1 blank cell and, while
        // paused, the marker reserves 8 + 1 more, so the segment's budget is width - 18.
        //
        // 120 columns: 22 + 69 = 91 <= 120 - 18 = 102, so nothing is cut.
        let l = line(&f, 120);
        assert!(l.contains("testhost (192.0.2.1)"), "{l:?}");
        assert!(l.contains("Snt 2"), "{l:?}");
        assert!(l.contains("[PAUSED]"), "{l:?}");
        assert!(l.ends_with("12:34:56"), "{l:?}");
        // 100 columns: the same 91 cells do not fit in 100 - 18 = 82, so the tail is cut …
        let l = line(&f, 100);
        assert!(!l.contains("Snt 2"), "left segment truncated: {l:?}");
        // … never at the clock's or the marker's expense.
        assert!(l.contains("[PAUSED]"), "{l:?}");
        assert!(l.ends_with("12:34:56"), "{l:?}");
        // 80 columns (the default terminal): only the first 80 - 18 = 62 cells of the left
        // segment survive, and the marker still sits one blank cell left of the clock.
        let l = line(&f, 80);
        assert!(l.starts_with("mtr-rs "), "{l:?}");
        assert!(!l.contains("Snt 2"), "left segment truncated: {l:?}");
        assert_eq!(l.chars().nth(62), Some(' '), "{l:?}");
        assert_eq!(&l.chars().skip(63).take(8).collect::<String>(), "[PAUSED]");
        assert_eq!(l.chars().nth(71), Some(' '), "{l:?}");
        assert!(l.ends_with("12:34:56"), "{l:?}");
    }

    /// Unpaused, the marker reserves nothing: the left segment keeps the full width - 9 budget.
    #[test]
    fn nothing_is_reserved_for_the_marker_while_running() {
        let f = view_fixture();
        let l = line(&f, 80);
        assert!(!l.contains("[PAUSED]"), "{l:?}");
        assert!(
            l.contains("Snt 2"),
            "the 69-cell segment fits in 80 - 9 = 71: {l:?}"
        );
        assert!(l.ends_with("12:34:56"), "{l:?}");
    }
}
