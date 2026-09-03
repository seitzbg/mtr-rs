//! Pure renderers over a `View` (spec §8). GPL-2.0-only.

pub mod detail; // Task 12
pub mod footer;
pub mod header;
pub mod help; // Task 13
pub mod sparkline;
pub mod table; // Task 11
pub mod too_small; // Task 13

use std::time::Instant;

use mtr_core::Engine;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::names::NameCache;
use crate::tui::glyphs::Glyphs;
use crate::tui::palette::Palette;
use crate::tui::state::{Bounds, UiState};

pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 12;
pub const PANE_MIN_HEIGHT: u16 = 20;
pub const DETAIL_ROWS: u16 = 9;

pub struct View<'a> {
    pub engine: &'a Engine,
    pub names: &'a NameCache,
    pub ui: &'a UiState,
    pub glyphs: &'static Glyphs,
    pub palette: &'a Palette,
    pub now: Instant,
    /// Wall clock `HH:MM:SS`, formatted by the caller (keeps the renderer deterministic).
    pub clock: &'a str,
    pub local_hostname: &'a str,
    pub target_name: &'a str,
    pub version: &'a str,
}

pub struct Areas {
    pub header: Rect,
    pub table: Rect,
    pub detail: Option<Rect>,
    pub footer: Rect,
}

pub fn pane_allowed(area: Rect) -> bool {
    area.height >= PANE_MIN_HEIGHT
}

pub fn layout(area: Rect, ui: &UiState) -> Areas {
    if ui.pane_open && pane_allowed(area) {
        let [header, table, detail, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(DETAIL_ROWS),
            Constraint::Length(1),
        ])
        .areas(area);
        Areas {
            header,
            table,
            detail: Some(detail),
            footer,
        }
    } else {
        let [header, table, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(area);
        Areas {
            header,
            table,
            detail: None,
            footer,
        }
    }
}

/// Hop rows the table can show (one line is the column header).
pub fn table_capacity(area: Rect, ui: &UiState) -> usize {
    usize::from(layout(area, ui).table.height.saturating_sub(1))
}

pub fn bounds(area: Rect, engine: &Engine, ui: &UiState) -> Bounds {
    Bounds {
        range: engine.display_range(),
        visible_rows: table_capacity(area, ui),
        pane_allowed: pane_allowed(area),
    }
}

/// Microseconds as `%.1f` milliseconds.
pub fn ms(us: i32) -> String {
    format!("{:.1}", f64::from(us) / 1000.0)
}

pub fn draw(frame: &mut Frame, view: &View) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        too_small::render(view, area, frame.buffer_mut());
        return;
    }
    let areas = layout(area, view.ui);
    let buf = frame.buffer_mut();
    header::render(view, areas.header, buf);
    table::render(view, areas.table, buf);
    if let Some(d) = areas.detail {
        detail::render(view, d, buf);
    }
    footer::render(view, areas.footer, buf);
    if let Some(p) = &view.ui.prompt {
        // display widths throughout (deviation 22), including the prefix
        let x = areas.footer.x
            + crate::width::display_width(&footer::prompt_prefix(view, p)) as u16
            + crate::width::display_width(&p.buf) as u16;
        frame.set_cursor_position((x.min(area.width.saturating_sub(1)), areas.footer.y));
    }
    if view.ui.help {
        help::render(view, area, frame.buffer_mut());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn layout_reserves_header_footer_and_the_pane_only_when_tall_enough() {
        let mut ui = UiState::new();
        let a = layout(Rect::new(0, 0, 80, 24), &ui);
        assert_eq!(a.header, Rect::new(0, 0, 80, 1));
        assert_eq!(a.footer, Rect::new(0, 23, 80, 1));
        assert_eq!(a.detail, Some(Rect::new(0, 14, 80, DETAIL_ROWS)));
        assert_eq!(a.table, Rect::new(0, 1, 80, 13));
        assert_eq!(table_capacity(Rect::new(0, 0, 80, 24), &ui), 12);
        let a = layout(Rect::new(0, 0, 80, 19), &ui);
        assert_eq!(a.detail, None, "auto-hidden below 20 rows");
        assert_eq!(a.table.height, 17);
        ui.pane_open = false;
        assert_eq!(layout(Rect::new(0, 0, 80, 24), &ui).detail, None);
        assert_eq!(table_capacity(Rect::new(0, 0, 80, 24), &ui), 21);
    }

    #[test]
    fn helpers() {
        // protocol_name lives in tui/mod.rs (Task 6) and is tested there
        assert_eq!(ms(500), "0.5");
        assert_eq!(ms(123_456), "123.5");
        assert_eq!(ms(0), "0.0");
    }
}
