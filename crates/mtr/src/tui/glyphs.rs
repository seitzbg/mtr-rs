//! Glyph sets: Unicode by default, `--ascii` fallback (spec §8 rendering rules). GPL-2.0-only.

use ratatui::symbols::{Marker, border};

pub struct Glyphs {
    /// Sparkline buckets, lowest RTT first.
    pub bars: [&'static str; 8],
    pub loss: &'static str,
    pub pending: &'static str,
    /// Selected-row marker in the table.
    pub selected: &'static str,
    /// Loss row under the RTT chart.
    pub lost_mark: &'static str,
    /// Header arrow between local host and target.
    pub arrow: &'static str,
    /// Separator between the detail pane's tab titles.
    pub divider: &'static str,
    /// Marks a value the detail pane had to cut short.
    pub ellipsis: &'static str,
    pub border: border::Set<'static>,
    pub marker: Marker,
    /// True for the `--ascii` set: renderers that cannot express themselves through the glyphs
    /// above (the RTT plot) switch to a pure-ASCII code path.
    pub ascii: bool,
}

pub static UNICODE: Glyphs = Glyphs {
    bars: ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"],
    loss: "░",
    pending: " ",
    selected: "▶",
    lost_mark: "×",
    arrow: "→",
    divider: "│",
    ellipsis: "…",
    border: border::ROUNDED,
    marker: Marker::Braille,
    ascii: false,
};

pub static ASCII: Glyphs = Glyphs {
    bars: [".", ":", "-", "=", "+", "*", "#", "@"],
    loss: "x",
    pending: " ",
    selected: ">",
    lost_mark: "x",
    arrow: "->",
    divider: "|",
    ellipsis: "~",
    border: border::Set {
        top_left: "+",
        top_right: "+",
        bottom_left: "+",
        bottom_right: "+",
        vertical_left: "|",
        vertical_right: "|",
        horizontal_top: "-",
        horizontal_bottom: "-",
    },
    marker: Marker::Dot,
    ascii: true,
};

impl Glyphs {
    pub fn select(ascii: bool) -> &'static Glyphs {
        if ascii { &ASCII } else { &UNICODE }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_have_eight_bars_and_ascii_is_pure_ascii() {
        assert_eq!(UNICODE.bars, ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"]);
        assert_eq!(UNICODE.loss, "░");
        for g in [
            ASCII.loss,
            ASCII.pending,
            ASCII.selected,
            ASCII.lost_mark,
            ASCII.arrow,
            ASCII.divider,
            ASCII.ellipsis,
        ] {
            assert!(g.is_ascii(), "{g:?}");
        }
        assert_eq!(UNICODE.divider, "│");
        assert_eq!((UNICODE.ellipsis, ASCII.ellipsis), ("…", "~"));
        assert!(ASCII.bars.iter().all(|b| b.is_ascii() && b.len() == 1));
        assert!(ASCII.ascii && !UNICODE.ascii);
        assert!(std::ptr::eq(Glyphs::select(true), &ASCII));
        assert!(std::ptr::eq(Glyphs::select(false), &UNICODE));
    }
}
