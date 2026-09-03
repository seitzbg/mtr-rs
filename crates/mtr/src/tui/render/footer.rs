//! Footer: key hints, transient status, or the active prompt (spec §8 item 4). GPL-2.0-only.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use crate::tui::render::View;
use crate::tui::state::{Prompt, PromptKind, UiState};

pub fn hints(ui: &UiState) -> String {
    if ui.help {
        return "any key closes help".to_string();
    }
    "q quit  p pause  SPACE resume  r reset  n dns  z asn  e mpls  d recent  ↑↓ select  Tab tab  Enter pane  ? help".to_string()
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
        Line::from(Span::styled(h, pal.dim()))
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
            row_text(&buf, 0).starts_with("q quit  p pause"),
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
}
