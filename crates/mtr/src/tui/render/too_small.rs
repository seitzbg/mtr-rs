//! "Terminal too small" placeholder (spec §8 item 6). Stub: filled in by Task 13. GPL-2.0-only.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::tui::render::View;

pub fn render(_view: &View, _area: Rect, _buf: &mut Buffer) {}
