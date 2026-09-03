//! "terminal too small" screen (spec §8 rendering rules). GPL-2.0-only.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::tui::render::{MIN_HEIGHT, MIN_WIDTH, View};

pub fn render(view: &View, area: Rect, buf: &mut Buffer) {
    let msg = format!(
        "terminal too small: {MIN_WIDTH}x{MIN_HEIGHT} minimum (now {}x{})",
        area.width, area.height
    );
    let x = area.x + area.width.saturating_sub(msg.len() as u16) / 2;
    let y = area.y + area.height / 2;
    buf.set_string(x, y, &msg, view.palette.alert());
}
