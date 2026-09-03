//! Interactive terminal UI (spec §8). Replaces ui/curses.c (mtr 0.96, commit 7b01773) with a
//! ratatui table + detail pane. GPL-2.0-only.

pub mod glyphs;
pub mod palette;
pub mod state;
pub mod terminal;

pub use glyphs::Glyphs;
pub use palette::{Depth, Palette};
pub use state::{Bounds, DetailTab, PromptKind, Quit, UiAction, UiState};

/// The single protocol-name mapping; used by the header (Task 10) and by
/// `input::toggle_status` (Task 8).
pub fn protocol_name(p: mtr_proto::Protocol) -> &'static str {
    match p {
        mtr_proto::Protocol::Icmp => "ICMP",
        mtr_proto::Protocol::Udp => "UDP",
        mtr_proto::Protocol::Tcp => "TCP",
        mtr_proto::Protocol::Sctp => "SCTP",
    }
}

#[cfg(test)]
mod tests {
    use mtr_proto::Protocol;

    #[test]
    fn protocol_names() {
        assert_eq!(super::protocol_name(Protocol::Icmp), "ICMP");
        assert_eq!(super::protocol_name(Protocol::Udp), "UDP");
        assert_eq!(super::protocol_name(Protocol::Tcp), "TCP");
        assert_eq!(super::protocol_name(Protocol::Sctp), "SCTP");
    }
}
