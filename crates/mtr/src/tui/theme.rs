//! Themes: the colours behind the semantic styles of [`super::palette::Palette`]. A theme is a
//! preset (`--theme NAME`, `theme.preset`) with per-role overrides from the `[theme]` section of
//! the config file. The `default` preset is the terminal's own named ANSI colours, so with no
//! theme configured the terminal decides the exact green/yellow/red; the other presets are the
//! usual RGB palettes, reduced to 256 colours when that is what the terminal offers and giving
//! way to the terminal's palette on 16 (see [`Theme::for_depth`]). GPL-2.0-only.

use std::str::FromStr;

use ratatui::style::Color;
use serde::Deserialize;

use super::palette::Depth;

/// The built-in presets. `Default` is the terminal's ANSI palette (the 0.3 look).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ThemeName {
    /// The terminal's own ANSI colours.
    #[default]
    Default,
    Dracula,
    Nord,
    Solarized,
    Gruvbox,
}

impl ThemeName {
    /// The spelling the config file uses, i.e. the `serde(rename_all = "lowercase")` name.
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeName::Default => "default",
            ThemeName::Dracula => "dracula",
            ThemeName::Nord => "nord",
            ThemeName::Solarized => "solarized",
            ThemeName::Gruvbox => "gruvbox",
        }
    }
}

/// One colour per semantic role. `Color::Reset` means "no colour": for `dim` and `selected` the
/// attribute alone (dim text, a reversed row); elsewhere plain text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// First ramp stop: no loss, fast RTT.
    pub ok: Color,
    /// Second ramp stop: some loss, RTT above the first threshold.
    pub warn: Color,
    /// Third RTT ramp stop (the loss ramp skips it, as C mtr does).
    pub bad: Color,
    /// Last ramp stop, bold at 100 % loss / above the last RTT threshold; also a lost sample.
    pub critical: Color,
    /// Headers (bold), the prompt prefix, the RTT chart.
    pub accent: Color,
    /// The status line, `[PAUSED]`, the "too small" message, no-route rows (bold).
    pub alert: Color,
    /// Unknown hops, secondary rows, axes, the footer hints; drawn dim as well.
    pub dim: Color,
    /// The selected row's background; `Reset` reverses the row instead.
    pub selected: Color,
}

/// The roles of the `[theme]` section, in file order. Shared by the config reader and writer so
/// the two cannot drift.
pub const ROLES: [&str; 8] = [
    "ok", "warn", "bad", "critical", "accent", "alert", "dim", "selected",
];

/// Per-role overrides from the config file; `None` keeps the preset's colour.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ThemeOverrides {
    pub ok: Option<Color>,
    pub warn: Option<Color>,
    pub bad: Option<Color>,
    pub critical: Option<Color>,
    pub accent: Option<Color>,
    pub alert: Option<Color>,
    pub dim: Option<Color>,
    pub selected: Option<Color>,
}

impl ThemeOverrides {
    /// The overrides as `(role, colour)` in [`ROLES`] order.
    pub fn entries(&self) -> [(&'static str, Option<Color>); 8] {
        [
            ("ok", self.ok),
            ("warn", self.warn),
            ("bad", self.bad),
            ("critical", self.critical),
            ("accent", self.accent),
            ("alert", self.alert),
            ("dim", self.dim),
            ("selected", self.selected),
        ]
    }
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Default for Theme {
    fn default() -> Self {
        Theme::preset(ThemeName::Default)
    }
}

impl Theme {
    pub fn preset(name: ThemeName) -> Theme {
        match name {
            ThemeName::Default => Theme {
                ok: Color::Green,
                warn: Color::Yellow,
                bad: Color::Magenta,
                critical: Color::Red,
                accent: Color::Blue,
                alert: Color::Yellow,
                dim: Color::Reset,
                selected: Color::Reset,
            },
            ThemeName::Dracula => Theme {
                ok: rgb(0x50fa7b),
                warn: rgb(0xf1fa8c),
                bad: rgb(0xffb86c),
                critical: rgb(0xff5555),
                accent: rgb(0xbd93f9),
                alert: rgb(0xff79c6),
                dim: rgb(0x6272a4),
                selected: rgb(0x44475a),
            },
            ThemeName::Nord => Theme {
                ok: rgb(0xa3be8c),
                warn: rgb(0xebcb8b),
                bad: rgb(0xd08770),
                critical: rgb(0xbf616a),
                accent: rgb(0x88c0d0),
                alert: rgb(0xebcb8b),
                dim: rgb(0x4c566a),
                selected: rgb(0x434c5e),
            },
            ThemeName::Solarized => Theme {
                ok: rgb(0x859900),
                warn: rgb(0xb58900),
                bad: rgb(0xcb4b16),
                critical: rgb(0xdc322f),
                accent: rgb(0x268bd2),
                alert: rgb(0xb58900),
                dim: rgb(0x586e75),
                selected: rgb(0x073642),
            },
            ThemeName::Gruvbox => Theme {
                ok: rgb(0xb8bb26),
                warn: rgb(0xfabd2f),
                bad: rgb(0xfe8019),
                critical: rgb(0xfb4934),
                accent: rgb(0x83a598),
                alert: rgb(0xfabd2f),
                dim: rgb(0x928374),
                selected: rgb(0x3c3836),
            },
        }
    }

    /// The preset with the file's overrides on top.
    pub fn with_overrides(mut self, o: &ThemeOverrides) -> Theme {
        let set = |slot: &mut Color, v: Option<Color>| {
            if let Some(c) = v {
                *slot = c;
            }
        };
        set(&mut self.ok, o.ok);
        set(&mut self.warn, o.warn);
        set(&mut self.bad, o.bad);
        set(&mut self.critical, o.critical);
        set(&mut self.accent, o.accent);
        set(&mut self.alert, o.alert);
        set(&mut self.dim, o.dim);
        set(&mut self.selected, o.selected);
        self
    }

    /// The theme's colour for a role name from [`ROLES`].
    pub fn role(&self, name: &str) -> Option<Color> {
        Some(match name {
            "ok" => self.ok,
            "warn" => self.warn,
            "bad" => self.bad,
            "critical" => self.critical,
            "accent" => self.accent,
            "alert" => self.alert,
            "dim" => self.dim,
            "selected" => self.selected,
            _ => return None,
        })
    }

    /// Every colour reduced to what `depth` can show. On 256 colours an RGB value becomes the
    /// nearest entry of the colour cube or grey ramp. On 16 colours there is no faithful
    /// rendering of an RGB palette, so an RGB value (or an index above 15) gives way to the
    /// `default` preset's colour for that role, while named colours and the indexes 0..=15 still
    /// apply; the `default` preset is therefore the same at every depth.
    pub fn for_depth(self, depth: Depth) -> Theme {
        let d = Theme::default();
        let q = |c, fallback| quantize(c, depth, fallback);
        Theme {
            ok: q(self.ok, d.ok),
            warn: q(self.warn, d.warn),
            bad: q(self.bad, d.bad),
            critical: q(self.critical, d.critical),
            accent: q(self.accent, d.accent),
            alert: q(self.alert, d.alert),
            dim: q(self.dim, d.dim),
            selected: q(self.selected, d.selected),
        }
    }
}

/// A colour as the config file spells it: a name (`green`, `light red`, `dark gray`), a
/// 256-colour index (`208`), `#rrggbb`, or `reset` for "no colour".
pub fn parse_color(s: &str) -> Result<Color, String> {
    Color::from_str(s.trim()).map_err(|_| {
        format!(
            "invalid colour {s:?}: expected a name such as \"green\", a 256-colour index, #rrggbb, or \"reset\""
        )
    })
}

/// The spelling [`parse_color`] reads back: names in lower case, indexes as numbers, RGB as
/// `#rrggbb`.
pub fn color_name(c: Color) -> String {
    c.to_string().to_lowercase()
}

fn quantize(c: Color, depth: Depth, fallback: Color) -> Color {
    match (depth, c) {
        (Depth::TrueColor | Depth::Mono, c) => c,
        (Depth::Ansi256, Color::Rgb(r, g, b)) => Color::Indexed(nearest_256(r, g, b)),
        (Depth::Ansi256, c) => c,
        (Depth::Ansi16, Color::Indexed(i)) if i < 16 => ANSI16[usize::from(i)],
        (Depth::Ansi16, Color::Rgb(..) | Color::Indexed(_)) => fallback,
        (Depth::Ansi16, c) => c,
    }
}

/// The 16 named colours in index order.
const ANSI16: [Color; 16] = [
    Color::Black,
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Magenta,
    Color::Cyan,
    Color::Gray,
    Color::DarkGray,
    Color::LightRed,
    Color::LightGreen,
    Color::LightYellow,
    Color::LightBlue,
    Color::LightMagenta,
    Color::LightCyan,
    Color::White,
];

fn dist(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let d = |x: u8, y: u8| {
        let d = i32::from(x) - i32::from(y);
        (d * d) as u32
    };
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

/// The xterm 256-colour layout above the named 16: a 6×6×6 cube at 16..=231, 24 greys at
/// 232..=255.
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn rgb_of_index(i: u8) -> (u8, u8, u8) {
    match i {
        0..=15 => unreachable!("only cube and grey indexes are compared"),
        16..=231 => {
            let n = i - 16;
            (
                CUBE[usize::from(n / 36)],
                CUBE[usize::from(n / 6 % 6)],
                CUBE[usize::from(n % 6)],
            )
        }
        _ => {
            let v = 8 + 10 * (i - 232);
            (v, v, v)
        }
    }
}

fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    let want = (r, g, b);
    let nearest_cube = |v: u8| {
        CUBE.iter()
            .enumerate()
            .min_by_key(|(_, c)| dist((**c, 0, 0), (v, 0, 0)))
            .map(|(i, _)| i as u8)
            .unwrap_or(0)
    };
    let cube = 16 + 36 * nearest_cube(r) + 6 * nearest_cube(g) + nearest_cube(b);
    let grey_level = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    let grey = 232 + (grey_level.saturating_sub(8).min(230) / 10) as u8;
    if dist(rgb_of_index(grey), want) < dist(rgb_of_index(cube), want) {
        grey
    } else {
        cube
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_preset_is_the_terminal_ansi_palette() {
        let t = Theme::default();
        assert_eq!(t.ok, Color::Green);
        assert_eq!(t.warn, Color::Yellow);
        assert_eq!(t.bad, Color::Magenta);
        assert_eq!(t.critical, Color::Red);
        assert_eq!(t.accent, Color::Blue);
        assert_eq!(t.alert, Color::Yellow);
        assert_eq!((t.dim, t.selected), (Color::Reset, Color::Reset));
        // and it is the same at every depth
        for d in [Depth::TrueColor, Depth::Ansi256, Depth::Ansi16, Depth::Mono] {
            assert_eq!(t.for_depth(d), t);
        }
    }

    #[test]
    fn every_role_name_resolves_and_the_presets_are_all_rgb() {
        for name in [
            ThemeName::Dracula,
            ThemeName::Nord,
            ThemeName::Solarized,
            ThemeName::Gruvbox,
        ] {
            let t = Theme::preset(name);
            for role in ROLES {
                assert!(
                    matches!(t.role(role), Some(Color::Rgb(..))),
                    "{name:?} {role}"
                );
            }
        }
        assert_eq!(Theme::default().role("nope"), None);
    }

    #[test]
    fn overrides_replace_only_the_roles_they_name() {
        let o = ThemeOverrides {
            critical: Some(Color::LightRed),
            selected: Some(Color::Indexed(238)),
            ..ThemeOverrides::default()
        };
        let t = Theme::default().with_overrides(&o);
        assert_eq!(t.critical, Color::LightRed);
        assert_eq!(t.selected, Color::Indexed(238));
        assert_eq!(t.ok, Color::Green);
        assert_eq!(
            Theme::default().with_overrides(&ThemeOverrides::default()),
            Theme::default()
        );
    }

    #[test]
    fn colours_parse_from_names_indexes_hex_and_reset_and_print_back() {
        for (text, color) in [
            ("green", Color::Green),
            ("Light Red", Color::LightRed),
            ("dark gray", Color::DarkGray),
            ("grey", Color::Gray),
            ("208", Color::Indexed(208)),
            ("#ff5555", Color::Rgb(0xff, 0x55, 0x55)),
            ("reset", Color::Reset),
        ] {
            assert_eq!(parse_color(text).unwrap(), color, "{text}");
            assert_eq!(parse_color(&color_name(color)).unwrap(), color, "{text}");
        }
        assert_eq!(color_name(Color::LightRed), "lightred");
        assert_eq!(color_name(Color::Rgb(0xff, 0x55, 0x55)), "#ff5555");
        assert_eq!(color_name(Color::Indexed(7)), "7");
        let err = parse_color("pinkish").unwrap_err();
        assert!(err.starts_with("invalid colour \"pinkish\""), "{err}");
        assert!(parse_color("256").is_err());
        assert!(parse_color("#12345").is_err());
    }

    #[test]
    fn rgb_reduces_to_the_nearest_256_colour_and_falls_back_on_16() {
        // cube corners and axis points land exactly
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(255, 255, 255), 231);
        assert_eq!(nearest_256(255, 0, 0), 196);
        assert_eq!(nearest_256(95, 135, 175), 67);
        // a mid grey prefers the grey ramp over the cube
        assert_eq!(nearest_256(128, 128, 128), 244);
        assert_eq!(rgb_of_index(244), (128, 128, 128));
        let t = Theme::preset(ThemeName::Dracula);
        assert_eq!(t.for_depth(Depth::TrueColor), t);
        for role in ROLES {
            assert!(
                matches!(
                    t.for_depth(Depth::Ansi256).role(role),
                    Some(Color::Indexed(16..))
                ),
                "{role}"
            );
        }
        // 16 colours cannot show an RGB palette: those roles fall back to the default preset,
        // so a Dracula terminal on 16 colours looks like the terminal's own palette
        assert_eq!(t.for_depth(Depth::Ansi16), Theme::default());
        // …while named colours and the first 16 indexes are kept (an index above 15 is not)
        let o = ThemeOverrides {
            ok: Some(Color::LightGreen),
            warn: Some(Color::Indexed(9)),
            bad: Some(Color::Indexed(208)),
            ..ThemeOverrides::default()
        };
        let t16 = t.with_overrides(&o).for_depth(Depth::Ansi16);
        assert_eq!(t16.ok, Color::LightGreen);
        assert_eq!(t16.warn, Color::LightRed);
        assert_eq!(t16.bad, Color::Magenta);
        assert_eq!(
            quantize(Color::Indexed(196), Depth::Ansi256, Color::Red),
            Color::Indexed(196)
        );
        assert_eq!(
            quantize(Color::Reset, Depth::Ansi16, Color::Red),
            Color::Reset
        );
    }
}
