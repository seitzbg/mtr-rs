//! Local UI state and its reducer: selection, scrolling, tabs, prompts, status, quit. The prompt
//! and scroll rules follow ui/curses.c:138-430 (mtr 0.96, commit 7b01773). GPL-2.0-only.

use std::ops::Range;
use std::time::{Duration, Instant};

pub const STATUS_TTL: Duration = Duration::from_secs(3);
pub const SCROLL_STEP: usize = 5;
pub const MAX_PROMPT_LEN: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailTab {
    Rtt,
    Addresses,
    Log,
}

impl DetailTab {
    pub fn next(self) -> Self {
        match self {
            DetailTab::Rtt => DetailTab::Addresses,
            DetailTab::Addresses => DetailTab::Log,
            DetailTab::Log => DetailTab::Rtt,
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            DetailTab::Rtt => "RTT",
            DetailTab::Addresses => "Addresses",
            DetailTab::Log => "Log",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    PacketSize,
    BitPattern,
    Interval,
    FirstTtl,
    MaxTtl,
    Fields,
    Tos,
}

impl PromptKind {
    /// The C prompt text (curses.c) without the current value.
    pub fn label(self) -> &'static str {
        match self {
            PromptKind::PacketSize => "Change Packet Size",
            PromptKind::BitPattern => "Ping Bit Pattern",
            PromptKind::Interval => "Interval",
            PromptKind::FirstTtl => "First TTL",
            PromptKind::MaxTtl => "Max TTL",
            PromptKind::Fields => "Fields",
            PromptKind::Tos => "Type of Service(tos)",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            PromptKind::PacketSize => "28-65535, < 0: random",
            PromptKind::BitPattern => "0-255, -1: random",
            PromptKind::Interval => "seconds",
            PromptKind::FirstTtl => "1-maxTTL",
            PromptKind::MaxTtl => "firstTTL-255",
            PromptKind::Fields => "LDRSNBAWVGJMXI and space",
            PromptKind::Tos => "0-255",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub kind: PromptKind,
    pub buf: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    pub until: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quit {
    Key,
    CtrlC,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiAction {
    SelectUp,
    SelectDown,
    ScrollUp,
    ScrollDown,
    TogglePane,
    NextTab,
    ToggleSparkline,
    ToggleHelp,
    /// Esc: close help or cancel the prompt.
    CloseOverlay,
    OpenPrompt(PromptKind),
    PromptChar(char),
    PromptBackspace,
    PromptCancel,
    PromptSubmit,
    Redraw,
    Status(String),
    Quit(Quit),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bounds {
    pub range: Range<usize>,
    pub visible_rows: usize,
    /// False when the terminal is too short for the detail pane.
    pub pane_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submitted {
    pub kind: PromptKind,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct UiState {
    pub selected: usize,
    pub scroll: usize,
    pub pane_open: bool,
    pub tab: DetailTab,
    pub sparkline: bool,
    pub help: bool,
    pub prompt: Option<Prompt>,
    pub status: Option<Status>,
    pub quit: Option<Quit>,
    pub redraw: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self::new()
    }
}

impl UiState {
    pub fn new() -> Self {
        Self::with_view(true, true)
    }

    /// The initial Recent-column and detail-pane visibility, which `display.sparkline` and
    /// `display.detail_pane` in the config file choose.
    pub fn with_view(sparkline: bool, pane_open: bool) -> Self {
        UiState {
            selected: 0,
            scroll: 0,
            pane_open,
            tab: DetailTab::Rtt,
            sparkline,
            help: false,
            prompt: None,
            status: None,
            quit: None,
            redraw: false,
        }
    }

    pub fn pane_visible(&self, b: &Bounds) -> bool {
        self.pane_open && b.pane_allowed
    }

    pub fn set_status(&mut self, text: impl Into<String>, now: Instant) {
        self.status = Some(Status {
            text: text.into(),
            until: now + STATUS_TTL,
        });
    }

    pub fn expire_status(&mut self, now: Instant) {
        if self.status.as_ref().is_some_and(|s| now >= s.until) {
            self.status = None;
        }
    }

    /// Keep `selected` inside `range` and `scroll` such that the selection is visible.
    pub fn clamp(&mut self, b: &Bounds) {
        let (lo, hi) = (b.range.start, b.range.end);
        self.selected = if hi > lo {
            self.selected.clamp(lo, hi - 1)
        } else {
            lo
        };
        let rows = b.visible_rows.max(1);
        let max_scroll = hi.saturating_sub(rows).max(lo);
        self.scroll = self.scroll.clamp(lo, max_scroll);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
        // invariant every `apply` restores: the scroll offset names a hop inside `range`, and the
        // selection is on screen. `render::table::first_row_of` relies on it.
        debug_assert!(self.scroll >= lo && (hi <= lo || self.scroll <= max_scroll));
    }

    pub fn apply(&mut self, a: UiAction, b: &Bounds, now: Instant) -> Option<Submitted> {
        match a {
            UiAction::SelectUp => self.selected = self.selected.saturating_sub(1),
            UiAction::SelectDown => self.selected += 1,
            UiAction::ScrollDown => {
                let rows = b.visible_rows.max(1);
                let max_scroll = b.range.end.saturating_sub(rows).max(b.range.start);
                self.scroll = (self.scroll + SCROLL_STEP).min(max_scroll);
                self.selected = self.selected.max(self.scroll);
            }
            UiAction::ScrollUp => {
                self.scroll = self.scroll.saturating_sub(SCROLL_STEP).max(b.range.start);
                let rows = b.visible_rows.max(1);
                self.selected = self.selected.min(self.scroll + rows - 1);
            }
            UiAction::TogglePane => self.pane_open = !self.pane_open,
            UiAction::NextTab => self.tab = self.tab.next(),
            UiAction::ToggleSparkline => self.sparkline = !self.sparkline,
            UiAction::ToggleHelp => self.help = !self.help,
            UiAction::CloseOverlay => {
                self.help = false;
                self.prompt = None;
            }
            UiAction::OpenPrompt(kind) => {
                self.help = false;
                self.prompt = Some(Prompt {
                    kind,
                    buf: String::new(),
                });
            }
            UiAction::PromptChar(c) => {
                if let Some(p) = &mut self.prompt {
                    if p.buf.chars().count() < MAX_PROMPT_LEN {
                        p.buf.push(c);
                    }
                }
            }
            UiAction::PromptBackspace => {
                if let Some(p) = &mut self.prompt {
                    p.buf.pop();
                }
            }
            UiAction::PromptCancel => self.prompt = None,
            UiAction::PromptSubmit => {
                if let Some(p) = self.prompt.take() {
                    return Some(Submitted {
                        kind: p.kind,
                        text: p.buf,
                    });
                }
            }
            UiAction::Redraw => self.redraw = true,
            UiAction::Status(s) => self.set_status(s, now),
            UiAction::Quit(q) => self.quit = Some(q),
        }
        self.clamp(b);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(range: std::ops::Range<usize>, rows: usize) -> Bounds {
        Bounds {
            range,
            visible_rows: rows,
            pane_allowed: true,
        }
    }

    #[test]
    fn selection_is_clamped_to_the_display_range_and_scroll_follows() {
        let mut ui = UiState::new();
        let now = Instant::now();
        let bounds = b(0..10, 4);
        for _ in 0..20 {
            ui.apply(UiAction::SelectDown, &bounds, now);
        }
        assert_eq!(ui.selected, 9);
        assert_eq!(ui.scroll, 6, "last page starts at 9 - 4 + 1");
        for _ in 0..20 {
            ui.apply(UiAction::SelectUp, &bounds, now);
        }
        assert_eq!((ui.selected, ui.scroll), (0, 0));
        // first_ttl 3: range starts at 2
        ui.clamp(&b(2..5, 4));
        assert_eq!((ui.selected, ui.scroll), (2, 2));
        ui.clamp(&b(2..2, 4));
        assert_eq!(
            ui.selected, 2,
            "empty range parks the selection at its start"
        );
    }

    #[test]
    fn scroll_moves_by_five_and_clamps_like_curses() {
        let mut ui = UiState::new();
        let now = Instant::now();
        let bounds = b(0..12, 4);
        ui.apply(UiAction::ScrollDown, &bounds, now);
        assert_eq!(ui.scroll, 5);
        assert_eq!(ui.selected, 5, "selection stays on screen");
        ui.apply(UiAction::ScrollDown, &bounds, now);
        assert_eq!(ui.scroll, 8, "cannot scroll past the last page");
        ui.apply(UiAction::ScrollUp, &bounds, now);
        assert_eq!(ui.scroll, 3);
        ui.apply(UiAction::ScrollUp, &bounds, now);
        assert_eq!(ui.scroll, 0);
    }

    #[test]
    fn pane_tabs_sparkline_help_and_quit() {
        let mut ui = UiState::new();
        let now = Instant::now();
        let bounds = b(0..3, 10);
        assert!(ui.pane_visible(&bounds));
        ui.apply(UiAction::TogglePane, &bounds, now);
        assert!(!ui.pane_open);
        assert!(!ui.pane_visible(&Bounds {
            pane_allowed: false,
            ..b(0..3, 10)
        }));
        ui.apply(UiAction::NextTab, &bounds, now);
        assert_eq!(ui.tab, DetailTab::Addresses);
        ui.apply(UiAction::NextTab, &bounds, now);
        ui.apply(UiAction::NextTab, &bounds, now);
        assert_eq!(ui.tab, DetailTab::Rtt);
        ui.apply(UiAction::ToggleSparkline, &bounds, now);
        assert!(!ui.sparkline);
        ui.apply(UiAction::ToggleHelp, &bounds, now);
        assert!(ui.help);
        ui.apply(UiAction::CloseOverlay, &bounds, now);
        assert!(!ui.help);
        ui.apply(UiAction::Redraw, &bounds, now);
        assert!(ui.redraw);
        ui.apply(UiAction::Quit(Quit::CtrlC), &bounds, now);
        assert_eq!(ui.quit, Some(Quit::CtrlC));
    }

    #[test]
    fn prompt_editing_submits_and_cancels() {
        let mut ui = UiState::new();
        let now = Instant::now();
        let bounds = b(0..3, 10);
        ui.apply(UiAction::OpenPrompt(PromptKind::PacketSize), &bounds, now);
        for c in "1x00".chars() {
            ui.apply(UiAction::PromptChar(c), &bounds, now);
        }
        ui.apply(UiAction::PromptBackspace, &bounds, now);
        ui.apply(UiAction::PromptBackspace, &bounds, now);
        ui.apply(UiAction::PromptBackspace, &bounds, now);
        for c in "00".chars() {
            ui.apply(UiAction::PromptChar(c), &bounds, now);
        }
        assert_eq!(ui.prompt.as_ref().unwrap().buf, "100");
        let s = ui.apply(UiAction::PromptSubmit, &bounds, now).unwrap();
        assert_eq!((s.kind, s.text.as_str()), (PromptKind::PacketSize, "100"));
        assert!(ui.prompt.is_none());
        ui.apply(UiAction::OpenPrompt(PromptKind::Fields), &bounds, now);
        for c in "L".repeat(30).chars() {
            ui.apply(UiAction::PromptChar(c), &bounds, now);
        }
        assert_eq!(
            ui.prompt.as_ref().unwrap().buf.len(),
            MAX_PROMPT_LEN,
            "MAXFLD"
        );
        assert!(ui.apply(UiAction::PromptCancel, &bounds, now).is_none());
        assert!(ui.prompt.is_none());
        // CloseOverlay also cancels a prompt
        ui.apply(UiAction::OpenPrompt(PromptKind::Tos), &bounds, now);
        ui.apply(UiAction::CloseOverlay, &bounds, now);
        assert!(ui.prompt.is_none());
    }

    #[test]
    fn status_messages_expire_after_three_seconds() {
        let mut ui = UiState::new();
        let now = Instant::now();
        ui.set_status("DNS off", now);
        ui.expire_status(now + Duration::from_secs(2));
        assert_eq!(ui.status.as_ref().map(|s| s.text.as_str()), Some("DNS off"));
        ui.expire_status(now + STATUS_TTL);
        assert!(ui.status.is_none());
        ui.apply(UiAction::Status("x".into()), &b(0..1, 5), now);
        assert!(ui.status.is_some());
    }
}
