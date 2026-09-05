//! Colour depth ladder and the semantic styles of the TUI (spec §8). The colours come from a
//! [`Theme`]; the default one is the terminal's own named ANSI colours, so out of the box the
//! terminal decides the exact green/yellow/red and the display looks the same locally and over
//! ssh (which drops `COLORTERM`). The depth decides colour versus mono, and how far an RGB theme
//! is reduced (see [`Theme::for_depth`]).
//! Ported from ui/curses.c (mtr 0.96, commit 7b01773). GPL-2.0-only.

use ratatui::style::{Color, Modifier, Style};

use super::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    TrueColor,
    Ansi256,
    Ansi16,
    Mono,
}

/// The four upper bounds, in microseconds, that split the RTT colour ramp into green / yellow /
/// magenta / red / bold red. Configurable through `--rtt-thresholds` and `display.rtt_thresholds_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RttThresholds {
    pub us: [u32; 4],
}

pub const DEFAULT_RTT_THRESHOLDS_MS: [u64; 4] = [30, 100, 200, 500];

impl Default for RttThresholds {
    fn default() -> Self {
        RttThresholds::from_millis(&DEFAULT_RTT_THRESHOLDS_MS).expect("valid defaults")
    }
}

impl RttThresholds {
    /// Exactly four values, each positive, strictly ascending, and small enough to fit a `u32` of
    /// microseconds (the unit every RTT in the engine is kept in).
    pub fn from_millis(ms: &[u64]) -> Result<Self, String> {
        if ms.len() != 4 {
            return Err(format!(
                "rtt thresholds need exactly 4 values, got {}",
                ms.len()
            ));
        }
        let mut us = [0u32; 4];
        for (i, &m) in ms.iter().enumerate() {
            if m == 0 {
                return Err("rtt thresholds must be positive".to_string());
            }
            if i > 0 && m <= ms[i - 1] {
                return Err(format!(
                    "rtt thresholds must be ascending: {} is not greater than {}",
                    m,
                    ms[i - 1]
                ));
            }
            us[i] = u32::try_from(m.saturating_mul(1000))
                .map_err(|_| format!("rtt threshold out of range: {m}"))?;
        }
        Ok(RttThresholds { us })
    }

    pub fn to_millis(self) -> [u64; 4] {
        self.us.map(|u| u64::from(u) / 1000)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub depth: Depth,
    pub rtt_thresholds: RttThresholds,
    /// Already reduced to `depth` (see [`Palette::with_theme`]).
    pub theme: Theme,
}

impl Palette {
    pub fn new(depth: Depth) -> Self {
        Palette {
            depth,
            rtt_thresholds: RttThresholds::default(),
            theme: Theme::default(),
        }
    }

    /// Builder used by `run_target`; keeps `new`/`detect` unchanged for every other caller.
    pub fn with_rtt_thresholds(mut self, t: RttThresholds) -> Self {
        self.rtt_thresholds = t;
        self
    }

    /// The theme, reduced to what this palette's depth can show.
    pub fn with_theme(mut self, t: Theme) -> Self {
        self.theme = t.for_depth(self.depth);
        self
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

    /// The single place a colour becomes a style: nothing in Mono, and nothing for `Reset` (the
    /// theme's "no colour").
    fn fg(&self, color: Color) -> Style {
        let s = Style::new();
        match (self.depth, color) {
            (Depth::Mono, _) | (_, Color::Reset) => s,
            (Depth::Ansi16 | Depth::Ansi256 | Depth::TrueColor, c) => s.fg(c),
        }
    }

    /// Loss% cell colour; `permille` is `Hop::loss()` (per-mille ×100, i.e. 100 000 = 100 %).
    /// The ramp skips the theme's `bad` stop, as C mtr's does.
    pub fn loss(&self, permille: i32) -> Style {
        let t = &self.theme;
        match permille {
            p if p <= 0 => self.fg(t.ok),
            p if p < 10_000 => self.fg(t.warn),
            p if p >= 100_000 => self.fg(t.critical).add_modifier(Modifier::BOLD),
            _ => self.fg(t.critical),
        }
    }

    /// RTT cell/bar colour on absolute, path-independent thresholds (deviation 25). The defaults
    /// are < 30 ms green, < 100 ms yellow, < 200 ms magenta, < 500 ms red, >= 500 ms bold red;
    /// `display.rtt_thresholds_ms` / `--rtt-thresholds` replace the four bounds.
    pub fn rtt(&self, us: u32) -> Style {
        let t = self.rtt_thresholds.us;
        let c = &self.theme;
        match us {
            u if u < t[0] => self.fg(c.ok),
            u if u < t[1] => self.fg(c.warn),
            u if u < t[2] => self.fg(c.bad),
            u if u < t[3] => self.fg(c.critical),
            _ => self.fg(c.critical).add_modifier(Modifier::BOLD),
        }
    }

    pub fn dim(&self) -> Style {
        self.fg(self.theme.dim).add_modifier(Modifier::DIM)
    }
    pub fn bold(&self) -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }
    /// A background from the theme, or the reversed row when it has none (and always in Mono,
    /// where a background would be the only colour on screen).
    pub fn selected(&self) -> Style {
        match (self.depth, self.theme.selected) {
            (Depth::Mono, _) | (_, Color::Reset) => Style::new().add_modifier(Modifier::REVERSED),
            (_, bg) => Style::new().bg(bg),
        }
    }
    pub fn header(&self) -> Style {
        self.fg(self.theme.accent).add_modifier(Modifier::BOLD)
    }
    pub fn accent(&self) -> Style {
        self.fg(self.theme.accent)
    }
    pub fn alert(&self) -> Style {
        self.fg(self.theme.alert).add_modifier(Modifier::BOLD)
    }
    /// A single lost probe in the sparkline column on a hop that does answer: plain, not the
    /// bold 100 %-loss style (that would visually equate one drop with total loss).
    pub fn lost_sample(&self) -> Style {
        self.fg(self.theme.critical)
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
    fn lost_sample_is_plain_red_not_bold() {
        let p = Palette::new(Depth::Ansi16);
        assert_eq!(p.lost_sample().fg, Some(Color::Red));
        assert!(!p.lost_sample().add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn mono_has_no_colours_but_keeps_attributes() {
        let p = Palette::new(Depth::Mono);
        assert_eq!(p.loss(100_000).fg, None);
        assert!(p.loss(100_000).add_modifier.contains(Modifier::BOLD));
        assert_eq!(p.rtt(999_999).fg, None);
        assert!(p.selected().add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn threshold_validation() {
        assert_eq!(
            RttThresholds::from_millis(&[30, 100, 200]).unwrap_err(),
            "rtt thresholds need exactly 4 values, got 3"
        );
        assert_eq!(
            RttThresholds::from_millis(&[0, 100, 200, 500]).unwrap_err(),
            "rtt thresholds must be positive"
        );
        assert_eq!(
            RttThresholds::from_millis(&[30, 30, 200, 500]).unwrap_err(),
            "rtt thresholds must be ascending: 30 is not greater than 30"
        );
        assert_eq!(
            RttThresholds::from_millis(&[30, 100, 200, 5_000_000]).unwrap_err(),
            "rtt threshold out of range: 5000000"
        );
        let t = RttThresholds::from_millis(&[5, 10, 20, 40]).unwrap();
        assert_eq!(t.us, [5_000, 10_000, 20_000, 40_000]);
        assert_eq!(t.to_millis(), [5, 10, 20, 40]);
        assert_eq!(RttThresholds::default().to_millis(), [30, 100, 200, 500]);
    }

    #[test]
    fn rtt_colours_follow_custom_thresholds() {
        let p = Palette::new(Depth::Ansi16)
            .with_rtt_thresholds(RttThresholds::from_millis(&[1, 2, 3, 4]).unwrap());
        assert_eq!(p.rtt(999).fg, Some(Color::Green));
        assert_eq!(p.rtt(1_000).fg, Some(Color::Yellow));
        assert_eq!(p.rtt(2_000).fg, Some(Color::Magenta));
        assert_eq!(p.rtt(3_000).fg, Some(Color::Red));
        assert!(!p.rtt(3_999).add_modifier.contains(Modifier::BOLD));
        assert!(p.rtt(4_000).add_modifier.contains(Modifier::BOLD));
        // the default palette is unaffected
        assert_eq!(
            Palette::new(Depth::Ansi16).rtt(1_000).fg,
            Some(Color::Green)
        );
    }

    #[test]
    fn a_theme_recolours_every_role_and_is_reduced_to_the_depth() {
        use crate::tui::theme::{Theme, ThemeName, ThemeOverrides};
        let dracula = Theme::preset(ThemeName::Dracula);
        let p = Palette::new(Depth::TrueColor).with_theme(dracula);
        assert_eq!(p.loss(0).fg, Some(dracula.ok));
        assert_eq!(p.rtt(150_000).fg, Some(dracula.bad));
        assert_eq!(p.lost_sample().fg, Some(dracula.critical));
        assert_eq!(p.header().fg, Some(dracula.accent));
        assert!(p.header().add_modifier.contains(Modifier::BOLD));
        assert_eq!(p.alert().fg, Some(dracula.alert));
        assert_eq!(p.dim().fg, Some(dracula.dim));
        assert!(p.dim().add_modifier.contains(Modifier::DIM));
        // a themed selection is a background, not a reversed row
        assert_eq!(p.selected().bg, Some(dracula.selected));
        assert!(!p.selected().add_modifier.contains(Modifier::REVERSED));
        // on 16 colours an RGB preset gives way to the terminal's palette
        let p16 = Palette::new(Depth::Ansi16).with_theme(dracula);
        assert_eq!(p16.loss(100_000).fg, Some(Color::Red));
        assert!(p16.selected().add_modifier.contains(Modifier::REVERSED));
        // on 256 colours it is reduced to the cube
        let p256 = Palette::new(Depth::Ansi256).with_theme(dracula);
        assert!(matches!(p256.loss(100_000).fg, Some(Color::Indexed(_))));
        assert!(matches!(p256.selected().bg, Some(Color::Indexed(_))));
        // mono keeps only the attributes, whatever the theme says
        let mono = Palette::new(Depth::Mono).with_theme(dracula);
        assert_eq!(mono.loss(0).fg, None);
        assert_eq!(mono.selected().bg, None);
        assert!(mono.selected().add_modifier.contains(Modifier::REVERSED));
        // `reset` in a role means no colour, and the default theme's dim/selected are exactly that
        let d = Palette::new(Depth::TrueColor);
        assert_eq!(d.dim().fg, None);
        assert!(d.selected().add_modifier.contains(Modifier::REVERSED));
        let o = ThemeOverrides {
            ok: Some(Color::Reset),
            ..ThemeOverrides::default()
        };
        assert_eq!(
            Palette::new(Depth::Ansi16)
                .with_theme(Theme::default().with_overrides(&o))
                .loss(0)
                .fg,
            None
        );
    }

    #[test]
    fn rtt_colours_follow_fixed_absolute_thresholds() {
        let p = Palette::new(Depth::Ansi16);
        assert_eq!(p.rtt(0).fg, Some(Color::Green));
        assert_eq!(p.rtt(29_999).fg, Some(Color::Green));
        assert_eq!(p.rtt(30_000).fg, Some(Color::Yellow));
        assert_eq!(p.rtt(99_999).fg, Some(Color::Yellow));
        assert_eq!(p.rtt(100_000).fg, Some(Color::Magenta));
        assert_eq!(p.rtt(199_999).fg, Some(Color::Magenta));
        assert_eq!(p.rtt(200_000).fg, Some(Color::Red));
        assert!(!p.rtt(200_000).add_modifier.contains(Modifier::BOLD));
        assert_eq!(p.rtt(499_999).fg, Some(Color::Red));
        assert!(!p.rtt(499_999).add_modifier.contains(Modifier::BOLD));
        assert_eq!(p.rtt(500_000).fg, Some(Color::Red));
        assert!(p.rtt(500_000).add_modifier.contains(Modifier::BOLD));
    }
}
