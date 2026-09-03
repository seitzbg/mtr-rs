//! Help overlay: the key list of ui/curses.c:389-420 (mtr 0.96, commit 7b01773), adapted to the
//! TUI's bindings. GPL-2.0-only.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Widget, Wrap};

use crate::tui::render::View;

/// Every line here must fit in the help box's 70-column interior (`min(area.width-4, 72) - 2`
/// border columns) so `Paragraph` never has to wrap the two-column key layout; see the unit test
/// below. `o str` doesn't fit alongside `u|t` at that width, so it gets its own line instead of
/// having its `(default LS NABWV)` clipped.
pub const HELP: &str = "\
?|h     help                       q       quit
p       pause (SPACE to resume)    r       reset all counters
n       toggle DNS on/off          z|y     toggle ASN info on/off
e       toggle MPLS on/off         d       toggle Recent column
u|t     cycle ICMP/UDP/TCP
o str   set the columns (default LS NABWV)
i <n>   interval in seconds        f <n>   first TTL
m <n>   max TTL                    s <n>   packet size (n<0: random)
b <c>   bit pattern (-1: random)   Q <t>   TOS
Up/Dn j/k  select hop              Enter   toggle detail pane
Tab     next detail tab            +/- PgUp/PgDn  scroll
Ctrl-L  redraw

press any key to go back...";

pub fn render(view: &View, area: Rect, buf: &mut Buffer) {
    let w = area.width.saturating_sub(4).min(72);
    let h = area.height.saturating_sub(2).min(16);
    let box_area = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    Clear.render(box_area, buf);
    let block = Block::bordered()
        .border_set(view.glyphs.border)
        .title(Line::from(Span::styled(" Keys ", view.palette.header())));
    // Fallback for boxes narrower than 70 interior columns (e.g. the 60x12 spec-minimum
    // terminal): wrap rather than silently clip.
    Paragraph::new(HELP)
        .block(block)
        .wrap(Wrap { trim: false })
        .render(box_area, buf);
}

#[cfg(test)]
mod tests {
    use super::HELP;
    use crate::width::display_width;

    #[test]
    fn every_help_line_fits_the_70_column_box_interior() {
        for line in HELP.lines() {
            assert!(
                display_width(line) <= 70,
                "line {line:?} is {} columns wide, want <= 70",
                display_width(line)
            );
        }
    }
}
