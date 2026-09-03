//! Footer: key hints, transient status, or the active prompt (spec §8 item 4). GPL-2.0-only.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use crate::tui::render::View;
use crate::tui::state::{Prompt, PromptKind, UiState};
use crate::width::{display_width, truncate_to};

/// Hints most-useful-first: the full line is ~100 cells, so at 80 columns the tail is dropped and
/// whatever survives has to be enough to reach everything else — `q quit` and `? help` lead.
pub fn hints(ui: &UiState) -> String {
    if ui.help {
        return "any key closes help".to_string();
    }
    "q quit  ? help  p pause  Space resume  r reset  Enter pane  Tab tab  ↑↓ select  d recent  n dns  z asn  e mpls".to_string()
}

/// Drop whole hints (never half a word) until the line fits `width` display cells.
pub fn fit_hints(hints: &str, width: usize) -> String {
    if display_width(hints) <= width {
        return hints.to_string();
    }
    let mut out = String::new();
    for hint in hints.split("  ") {
        let sep = if out.is_empty() { 0 } else { 2 };
        if display_width(&out) + sep + display_width(hint) > width {
            break;
        }
        if sep != 0 {
            out.push_str("  ");
        }
        out.push_str(hint);
    }
    if out.is_empty() {
        // narrower than the first hint: cut it rather than print nothing
        return truncate_to(hints, width).to_string();
    }
    out
}

/// Current value of the prompted setting, as C prints it in the prompt line.
fn current_value(view: &View, kind: PromptKind) -> String {
    let c = view.engine.config();
    match kind {
        PromptKind::PacketSize => c.packet_size.to_string(),
        PromptKind::BitPattern => c.bit_pattern.to_string(),
        PromptKind::Interval => c.interval.to_string(),
        PromptKind::FirstTtl => c.first_ttl.to_string(),
        PromptKind::MaxTtl => c.max_ttl.to_string(),
        PromptKind::Fields => c.fields.clone(),
        PromptKind::Tos => c.tos.to_string(),
    }
}

/// `"<label> [<current>] (<hint>): "` — the text before the typed buffer.
pub fn prompt_prefix(view: &View, p: &Prompt) -> String {
    format!(
        "{} [{}] ({}): ",
        p.kind.label(),
        current_value(view, p.kind),
        p.kind.hint()
    )
}

pub fn render(view: &View, area: Rect, buf: &mut Buffer) {
    let pal = view.palette;
    let line = if let Some(p) = &view.ui.prompt {
        Line::from(vec![
            Span::styled(prompt_prefix(view, p), pal.accent()),
            Span::styled(p.buf.clone(), pal.bold()),
        ])
    } else if let Some(s) = &view.ui.status {
        Line::from(Span::styled(s.text.clone(), pal.alert()))
    } else {
        let mut h = hints(view.ui);
        if view.glyphs.arrow == "->" {
            h = h.replace("↑↓", "Up/Dn");
        }
        Line::from(Span::styled(
            fit_hints(&h, usize::from(area.width)),
            pal.dim(),
        ))
    };
    buf.set_line(area.x, area.y, &line, area.width);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{row_text, view_fixture};
    use crate::tui::state::{Prompt, PromptKind};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    #[test]
    fn footer_shows_hints_status_or_prompt() {
        let mut f = view_fixture();
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        render(&f.view(), area, &mut buf);
        assert!(
            row_text(&buf, 0).starts_with("q quit  ? help  p pause"),
            "{:?}",
            row_text(&buf, 0)
        );
        f.ui.set_status("DNS off", f.now);
        let mut buf = Buffer::empty(area);
        render(&f.view(), area, &mut buf);
        assert!(row_text(&buf, 0).starts_with("DNS off"));
        f.ui.status = None;
        f.ui.prompt = Some(Prompt {
            kind: PromptKind::PacketSize,
            buf: "10".into(),
        });
        let mut buf = Buffer::empty(area);
        render(&f.view(), area, &mut buf);
        assert!(
            row_text(&buf, 0).starts_with("Change Packet Size [64] (28-65535, < 0: random): 10"),
            "{:?}",
            row_text(&buf, 0)
        );
        assert_eq!(
            hints(&UiState {
                help: true,
                ..UiState::new()
            }),
            "any key closes help"
        );
    }

    #[test]
    fn hints_are_cut_at_a_hint_boundary_and_keep_quit_and_help() {
        let full = hints(&UiState::new());
        assert!(display_width(&full) > 80, "{full:?}");
        let cut = fit_hints(&full, 80);
        assert!(display_width(&cut) <= 80, "{cut:?}");
        assert!(cut.starts_with("q quit  ? help"), "{cut:?}");
        assert!(!cut.ends_with(' ') && !cut.contains("  s"), "{cut:?}");
        // every surviving hint is whole
        for h in cut.split("  ") {
            assert!(full.split("  ").any(|f| f == h), "partial hint {h:?}");
        }
        assert_eq!(fit_hints(&full, 6), "q quit");
        assert_eq!(fit_hints("q quit", 3), "q q");
        assert_eq!(fit_hints("abc", 10), "abc");
    }
}
