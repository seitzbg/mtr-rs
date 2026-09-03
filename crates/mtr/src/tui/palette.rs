//! Colour depth ladder and the semantic styles of the TUI (spec §8).
//! Ported from ui/curses.c (mtr 0.96, commit 7b01773). GPL-2.0-only.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    TrueColor,
    Ansi256,
    Ansi16,
    Mono,
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub depth: Depth,
}

impl Palette {
    pub fn new(depth: Depth) -> Self {
        Palette { depth }
    }

    /// `color == false` (`--no-color` / `NO_COLOR`) → Mono; otherwise crossterm's colour count
    /// plus `COLORTERM` decide.
    pub fn detect(color: bool) -> Self {
        if !color {
            return Palette::new(Depth::Mono);
        }
        let colorterm = std::env::var("COLORTERM").ok();
        Palette::new(Self::depth_from(
            crossterm::style::available_color_count(),
            colorterm.as_deref(),
        ))
    }

    pub fn depth_from(color_count: u16, colorterm: Option<&str>) -> Depth {
        match colorterm {
            Some(c) if c.eq_ignore_ascii_case("truecolor") || c.eq_ignore_ascii_case("24bit") => {
                Depth::TrueColor
            }
            _ if color_count >= 256 => Depth::Ansi256,
            _ => Depth::Ansi16,
        }
    }

    fn fg(&self, ansi: Color, idx: u8, rgb: (u8, u8, u8)) -> Style {
        let s = Style::new();
        match self.depth {
            Depth::Mono => s,
            Depth::Ansi16 => s.fg(ansi),
            Depth::Ansi256 => s.fg(Color::Indexed(idx)),
            Depth::TrueColor => s.fg(Color::Rgb(rgb.0, rgb.1, rgb.2)),
        }
    }

    fn green(&self) -> Style {
        self.fg(Color::Green, 114, (0x8e, 0xc0, 0x7c))
    }
    fn yellow(&self) -> Style {
        self.fg(Color::Yellow, 221, (0xe5, 0xc0, 0x7b))
    }
    fn magenta(&self) -> Style {
        self.fg(Color::Magenta, 176, (0xc6, 0x78, 0xdd))
    }
    fn red(&self) -> Style {
        self.fg(Color::Red, 203, (0xe0, 0x6c, 0x75))
    }
    fn blue(&self) -> Style {
        self.fg(Color::Blue, 75, (0x61, 0xaf, 0xef))
    }

    /// Loss% cell colour; `permille` is `Hop::loss()` (per-mille ×100, i.e. 100 000 = 100 %).
    pub fn loss(&self, permille: i32) -> Style {
        match permille {
            p if p <= 0 => self.green(),
            p if p < 10_000 => self.yellow(),
            p if p >= 100_000 => self.red().add_modifier(Modifier::BOLD),
            _ => self.red(),
        }
    }

    /// Bucket `b` (0 = fastest .. 7 = slowest) of the sparkline scale; the shape of `block_col`.
    pub fn bucket(&self, b: usize) -> Style {
        match b.min(7) {
            0..=2 => self.green(),
            3..=4 => self.yellow(),
            5 => self.magenta(),
            6 => self.red(),
            _ => self.red().add_modifier(Modifier::BOLD),
        }
    }

    pub fn dim(&self) -> Style {
        Style::new().add_modifier(Modifier::DIM)
    }
    pub fn bold(&self) -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }
    pub fn selected(&self) -> Style {
        Style::new().add_modifier(Modifier::REVERSED)
    }
    pub fn header(&self) -> Style {
        self.blue().add_modifier(Modifier::BOLD)
    }
    pub fn accent(&self) -> Style {
        self.blue()
    }
    pub fn alert(&self) -> Style {
        self.yellow().add_modifier(Modifier::BOLD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier};

    #[test]
    fn depth_ladder_from_color_count_and_colorterm() {
        assert_eq!(
            Palette::depth_from(256, Some("truecolor")),
            Depth::TrueColor
        );
        assert_eq!(Palette::depth_from(256, Some("24bit")), Depth::TrueColor);
        assert_eq!(Palette::depth_from(256, None), Depth::Ansi256);
        assert_eq!(Palette::depth_from(16, None), Depth::Ansi16);
        assert_eq!(Palette::depth_from(8, None), Depth::Ansi16);
        assert_eq!(Palette::detect(false).depth, Depth::Mono);
    }

    #[test]
    fn loss_colours_follow_the_spec_thresholds() {
        let p = Palette::new(Depth::Ansi16);
        assert_eq!(p.loss(0).fg, Some(Color::Green));
        assert_eq!(p.loss(5_000).fg, Some(Color::Yellow));
        assert_eq!(p.loss(9_999).fg, Some(Color::Yellow));
        assert_eq!(p.loss(10_000).fg, Some(Color::Red));
        assert!(!p.loss(10_000).add_modifier.contains(Modifier::BOLD));
        assert!(p.loss(100_000).add_modifier.contains(Modifier::BOLD));
        assert_eq!(p.loss(100_000).fg, Some(Color::Red));
    }

    #[test]
    fn mono_has_no_colours_but_keeps_attributes() {
        let p = Palette::new(Depth::Mono);
        assert_eq!(p.loss(100_000).fg, None);
        assert!(p.loss(100_000).add_modifier.contains(Modifier::BOLD));
        assert_eq!(p.bucket(7).fg, None);
        assert!(p.selected().add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn buckets_follow_block_col_shape() {
        // curses.c:91-102 collapsed to 8: green ×3, yellow ×2, magenta, red ×2
        let p = Palette::new(Depth::Ansi16);
        let fg: Vec<_> = (0..8).map(|b| p.bucket(b).fg.unwrap()).collect();
        assert_eq!(
            fg,
            [
                Color::Green,
                Color::Green,
                Color::Green,
                Color::Yellow,
                Color::Yellow,
                Color::Magenta,
                Color::Red,
                Color::Red
            ]
        );
        assert!(p.bucket(7).add_modifier.contains(Modifier::BOLD));
        assert_eq!(
            p.bucket(99).fg,
            Some(Color::Red),
            "out of range clamps to the top bucket"
        );
    }
}
