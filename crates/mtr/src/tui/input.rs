//! Keyboard handling: crossterm key → engine `UserAction` or local `UiAction`, and the prompt
//! value rules of ui/curses.c:138-430 (mtr 0.96, commit 7b01773). GPL-2.0-only.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mtr_core::fields::{AVAILABLE_OPTIONS, validate_fields};
use mtr_core::{Config, UserAction};

use crate::cli::{packet_size_in_range, parse_c_long};
use crate::tui::protocol_name;
use crate::tui::state::{Prompt, PromptKind, Quit, UiAction, UiState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Engine(UserAction),
    Ui(UiAction),
    None,
}

pub fn map_key(key: KeyEvent, ui: &UiState) -> Input {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')) {
        return Input::Ui(UiAction::Quit(Quit::CtrlC));
    }
    if let Some(Prompt { kind, .. }) = &ui.prompt {
        return match key.code {
            KeyCode::Enter => Input::Ui(UiAction::PromptSubmit),
            KeyCode::Esc => Input::Ui(UiAction::PromptCancel),
            KeyCode::Backspace => Input::Ui(UiAction::PromptBackspace),
            KeyCode::Char(c) if !ctrl => {
                if *kind == PromptKind::Fields && !AVAILABLE_OPTIONS.contains(c) {
                    Input::None // curses.c:349: illegal character → beep, ignored
                } else {
                    Input::Ui(UiAction::PromptChar(c))
                }
            }
            _ => Input::None,
        };
    }
    if ui.help {
        return match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => Input::Ui(UiAction::Quit(Quit::Key)),
            _ => Input::Ui(UiAction::CloseOverlay),
        };
    }
    if ctrl {
        return match key.code {
            KeyCode::Char('d') => Input::Ui(UiAction::Quit(Quit::Key)),
            KeyCode::Char('s') => Input::Engine(UserAction::Pause),
            KeyCode::Char('q') => Input::Engine(UserAction::Resume),
            KeyCode::Char('l') => Input::Ui(UiAction::Redraw),
            _ => Input::None,
        };
    }
    match key.code {
        KeyCode::Char('Q') => Input::Ui(UiAction::OpenPrompt(PromptKind::Tos)), // before tolower
        KeyCode::Char(c) => match c.to_ascii_lowercase() {
            'q' => Input::Ui(UiAction::Quit(Quit::Key)),
            'p' => Input::Engine(UserAction::Pause),
            ' ' => Input::Engine(UserAction::Resume),
            'r' => Input::Engine(UserAction::Reset),
            'n' => Input::Engine(UserAction::ToggleDns),
            'z' | 'y' => Input::Engine(UserAction::ToggleAsn),
            'e' => Input::Engine(UserAction::ToggleMpls),
            'u' | 't' => Input::Engine(UserAction::CycleProtocol),
            's' => Input::Ui(UiAction::OpenPrompt(PromptKind::PacketSize)),
            'b' => Input::Ui(UiAction::OpenPrompt(PromptKind::BitPattern)),
            'i' => Input::Ui(UiAction::OpenPrompt(PromptKind::Interval)),
            'f' => Input::Ui(UiAction::OpenPrompt(PromptKind::FirstTtl)),
            'm' => Input::Ui(UiAction::OpenPrompt(PromptKind::MaxTtl)),
            'o' => Input::Ui(UiAction::OpenPrompt(PromptKind::Fields)),
            '?' | 'h' => Input::Ui(UiAction::ToggleHelp),
            '+' => Input::Ui(UiAction::ScrollDown),
            '-' => Input::Ui(UiAction::ScrollUp),
            'j' => Input::Ui(UiAction::SelectDown),
            'k' => Input::Ui(UiAction::SelectUp),
            'd' => Input::Ui(UiAction::ToggleSparkline),
            _ => Input::None,
        },
        KeyCode::PageDown => Input::Ui(UiAction::ScrollDown),
        KeyCode::PageUp => Input::Ui(UiAction::ScrollUp),
        KeyCode::Down => Input::Ui(UiAction::SelectDown),
        KeyCode::Up => Input::Ui(UiAction::SelectUp),
        KeyCode::Tab => Input::Ui(UiAction::NextTab),
        KeyCode::Enter => Input::Ui(UiAction::TogglePane),
        KeyCode::Esc => Input::Ui(UiAction::CloseOverlay),
        _ => Input::None,
    }
}

/// Deviation 17: CLI parsing and bounds; the messages are the CLI's where one exists.
pub fn parse_prompt(
    kind: PromptKind,
    raw: &str,
    cfg: &Config,
    is_root: bool,
) -> Result<UserAction, String> {
    let text = raw.trim();
    match kind {
        PromptKind::PacketSize => {
            let n = parse_c_long(text)?;
            packet_size_in_range(n)?;
            Ok(UserAction::SetPacketSize(n as i32))
        }
        PromptKind::BitPattern => {
            let n = parse_c_long(text)?;
            if !(-1..=255).contains(&n) {
                return Err(format!("value out of range (-1 - 255): {n}"));
            }
            Ok(UserAction::SetBitPattern(n as i32))
        }
        PromptKind::Tos => {
            let n = parse_c_long(text)?;
            if !(0..=255).contains(&n) {
                return Err(format!("value out of range (0 - 255): {n}"));
            }
            Ok(UserAction::SetTos(n as u8))
        }
        PromptKind::Interval => {
            let f: f64 = text
                .parse()
                .map_err(|_| format!("invalid argument: '{text}'"))?;
            if f.is_nan() || f <= 0.0 {
                return Err("wait time must be positive".to_string());
            }
            if !is_root && f < 1.0 {
                return Err("non-root users cannot request an interval < 1.0 seconds".to_string());
            }
            Ok(UserAction::SetInterval(
                (f * 1000.0).round().min(f64::from(u32::MAX)) as u32,
            ))
        }
        PromptKind::FirstTtl => {
            let n = parse_c_long(text)?;
            if n < 1 {
                return Err("first TTL must be at least 1".to_string());
            }
            if n > i64::from(cfg.max_ttl) {
                return Err(format!(
                    "firstTTL({n}) cannot be larger than maxTTL({}).",
                    cfg.max_ttl
                ));
            }
            Ok(UserAction::SetFirstTtl(n as u8))
        }
        PromptKind::MaxTtl => {
            let n = parse_c_long(text)?;
            if n < i64::from(cfg.first_ttl) {
                return Err(format!(
                    "maxTTL({n}) cannot be less than firstTTL({}).",
                    cfg.first_ttl
                ));
            }
            if n > 255 {
                return Err(format!("maxTTL({n}) cannot be larger than 255."));
            }
            Ok(UserAction::SetMaxTtl(n as u8))
        }
        PromptKind::Fields => {
            // untrimmed: a trailing space is a spacer column (curses.c:355 keeps the old value when empty)
            if raw.is_empty() {
                return Err("fields unchanged".to_string());
            }
            validate_fields(raw)?;
            Ok(UserAction::SetFields(raw.to_string()))
        }
    }
}

/// Footer status after an engine action, given the config *after* the action.
pub fn toggle_status(action: &UserAction, cfg_after: &Config) -> Option<String> {
    let on_off = |b: bool| if b { "on" } else { "off" };
    Some(match action {
        UserAction::ToggleDns => format!("DNS {}", on_off(cfg_after.dns)),
        UserAction::ToggleAsn => format!("ASN {}", on_off(!cfg_after.ipinfo_fields.is_empty())),
        UserAction::ToggleMpls => format!("MPLS {}", on_off(cfg_after.mpls)),
        UserAction::CycleProtocol => format!("Protocol: {}", protocol_name(cfg_after.protocol)),
        UserAction::Pause => "Paused".to_string(),
        UserAction::Resume => "Resumed".to_string(),
        UserAction::Reset => "Statistics reset".to_string(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use mtr_core::Config;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }
    fn ch(c: char) -> KeyEvent {
        k(KeyCode::Char(c))
    }

    #[test]
    fn c_compatible_keys_map_to_engine_actions() {
        let ui = UiState::new();
        assert_eq!(map_key(ch('p'), &ui), Input::Engine(UserAction::Pause));
        assert_eq!(map_key(ctrl('s'), &ui), Input::Engine(UserAction::Pause));
        assert_eq!(map_key(ch(' '), &ui), Input::Engine(UserAction::Resume));
        assert_eq!(map_key(ctrl('q'), &ui), Input::Engine(UserAction::Resume));
        assert_eq!(map_key(ch('r'), &ui), Input::Engine(UserAction::Reset));
        assert_eq!(
            map_key(ch('R'), &ui),
            Input::Engine(UserAction::Reset),
            "tolower"
        );
        assert_eq!(map_key(ch('n'), &ui), Input::Engine(UserAction::ToggleDns));
        assert_eq!(map_key(ch('z'), &ui), Input::Engine(UserAction::ToggleAsn));
        assert_eq!(map_key(ch('y'), &ui), Input::Engine(UserAction::ToggleAsn));
        assert_eq!(map_key(ch('e'), &ui), Input::Engine(UserAction::ToggleMpls));
        assert_eq!(
            map_key(ch('u'), &ui),
            Input::Engine(UserAction::CycleProtocol)
        );
        assert_eq!(
            map_key(ch('t'), &ui),
            Input::Engine(UserAction::CycleProtocol)
        );
    }

    #[test]
    fn ui_keys_prompts_and_quit() {
        let ui = UiState::new();
        assert_eq!(map_key(ch('q'), &ui), Input::Ui(UiAction::Quit(Quit::Key)));
        assert_eq!(
            map_key(ctrl('c'), &ui),
            Input::Ui(UiAction::Quit(Quit::CtrlC))
        );
        assert_eq!(
            map_key(ctrl('d'), &ui),
            Input::Ui(UiAction::Quit(Quit::Key))
        );
        assert_eq!(
            map_key(ch('s'), &ui),
            Input::Ui(UiAction::OpenPrompt(PromptKind::PacketSize))
        );
        assert_eq!(
            map_key(ch('b'), &ui),
            Input::Ui(UiAction::OpenPrompt(PromptKind::BitPattern))
        );
        assert_eq!(
            map_key(ch('i'), &ui),
            Input::Ui(UiAction::OpenPrompt(PromptKind::Interval))
        );
        assert_eq!(
            map_key(ch('f'), &ui),
            Input::Ui(UiAction::OpenPrompt(PromptKind::FirstTtl))
        );
        assert_eq!(
            map_key(ch('m'), &ui),
            Input::Ui(UiAction::OpenPrompt(PromptKind::MaxTtl))
        );
        assert_eq!(
            map_key(ch('o'), &ui),
            Input::Ui(UiAction::OpenPrompt(PromptKind::Fields))
        );
        assert_eq!(
            map_key(ch('Q'), &ui),
            Input::Ui(UiAction::OpenPrompt(PromptKind::Tos))
        );
        assert_eq!(map_key(ch('?'), &ui), Input::Ui(UiAction::ToggleHelp));
        assert_eq!(map_key(ch('h'), &ui), Input::Ui(UiAction::ToggleHelp));
        assert_eq!(map_key(ctrl('l'), &ui), Input::Ui(UiAction::Redraw));
        assert_eq!(map_key(ch('+'), &ui), Input::Ui(UiAction::ScrollDown));
        assert_eq!(
            map_key(k(KeyCode::PageDown), &ui),
            Input::Ui(UiAction::ScrollDown)
        );
        assert_eq!(map_key(ch('-'), &ui), Input::Ui(UiAction::ScrollUp));
        assert_eq!(
            map_key(k(KeyCode::PageUp), &ui),
            Input::Ui(UiAction::ScrollUp)
        );
        assert_eq!(
            map_key(k(KeyCode::Down), &ui),
            Input::Ui(UiAction::SelectDown)
        );
        assert_eq!(map_key(ch('j'), &ui), Input::Ui(UiAction::SelectDown));
        assert_eq!(map_key(k(KeyCode::Up), &ui), Input::Ui(UiAction::SelectUp));
        assert_eq!(map_key(ch('k'), &ui), Input::Ui(UiAction::SelectUp));
        assert_eq!(map_key(k(KeyCode::Tab), &ui), Input::Ui(UiAction::NextTab));
        assert_eq!(
            map_key(k(KeyCode::Enter), &ui),
            Input::Ui(UiAction::TogglePane)
        );
        assert_eq!(map_key(ch('d'), &ui), Input::Ui(UiAction::ToggleSparkline));
        assert_eq!(
            map_key(k(KeyCode::Esc), &ui),
            Input::Ui(UiAction::CloseOverlay)
        );
        assert_eq!(map_key(ch('x'), &ui), Input::None);
    }

    #[test]
    fn prompt_and_help_modes_capture_keys() {
        let mut ui = UiState::new();
        ui.prompt = Some(Prompt {
            kind: PromptKind::Fields,
            buf: String::new(),
        });
        assert_eq!(map_key(ch('L'), &ui), Input::Ui(UiAction::PromptChar('L')));
        assert_eq!(
            map_key(ch('Q'), &ui),
            Input::None,
            "not in available_options"
        );
        assert_eq!(map_key(ch(' '), &ui), Input::Ui(UiAction::PromptChar(' ')));
        ui.prompt = Some(Prompt {
            kind: PromptKind::PacketSize,
            buf: String::new(),
        });
        assert_eq!(
            map_key(ch('q'), &ui),
            Input::Ui(UiAction::PromptChar('q')),
            "not quit while typing"
        );
        assert_eq!(
            map_key(k(KeyCode::Backspace), &ui),
            Input::Ui(UiAction::PromptBackspace)
        );
        assert_eq!(
            map_key(k(KeyCode::Enter), &ui),
            Input::Ui(UiAction::PromptSubmit)
        );
        assert_eq!(
            map_key(k(KeyCode::Esc), &ui),
            Input::Ui(UiAction::PromptCancel)
        );
        assert_eq!(
            map_key(ctrl('c'), &ui),
            Input::Ui(UiAction::Quit(Quit::CtrlC))
        );
        let mut ui = UiState::new();
        ui.help = true;
        assert_eq!(map_key(ch('x'), &ui), Input::Ui(UiAction::CloseOverlay));
        assert_eq!(map_key(ch('q'), &ui), Input::Ui(UiAction::Quit(Quit::Key)));
    }

    #[test]
    fn prompt_values_follow_the_c_bounds_with_cli_parsing() {
        let cfg = Config::default();
        let p = |kind, s| parse_prompt(kind, s, &cfg, false);
        assert_eq!(
            p(PromptKind::PacketSize, "100"),
            Ok(UserAction::SetPacketSize(100))
        );
        assert_eq!(
            p(PromptKind::PacketSize, "-200"),
            Ok(UserAction::SetPacketSize(-200))
        );
        assert_eq!(
            p(PromptKind::PacketSize, "10"),
            Err("value out of range (28 - 65535)".into())
        );
        // `parse_c_long` parses the digits as a positive i64 before negating, so it already
        // rejects a magnitude of 2^63 (`i64::from_str_radix` can't represent it) — meaning
        // `packet_size_in_range` never actually sees `i64::MIN` through this path today. It's
        // still an end-to-end proof that this exact 20-char (MAX_PROMPT_LEN) input errors
        // cleanly rather than panicking.
        assert_eq!(
            p(PromptKind::PacketSize, "-9223372036854775808"),
            Err("invalid argument: '-9223372036854775808'".into())
        );
        // The real regression test for the `n.abs()` overflow: call the range check directly
        // with `i64::MIN`, which `n.abs()` cannot represent and would have panicked (debug) or
        // silently misbehaved (release) on.
        assert_eq!(
            crate::cli::packet_size_in_range(i64::MIN),
            Err("value out of range (28 - 65535)".into())
        );
        assert_eq!(
            p(PromptKind::BitPattern, "-1"),
            Ok(UserAction::SetBitPattern(-1))
        );
        assert_eq!(
            p(PromptKind::BitPattern, "256"),
            Err("value out of range (-1 - 255): 256".into())
        );
        assert_eq!(p(PromptKind::Tos, "0x10"), Ok(UserAction::SetTos(16)));
        assert_eq!(
            p(PromptKind::Tos, "300"),
            Err("value out of range (0 - 255): 300".into())
        );
        assert_eq!(
            p(PromptKind::Interval, "2.5"),
            Ok(UserAction::SetInterval(2500))
        );
        assert_eq!(
            p(PromptKind::Interval, "0"),
            Err("wait time must be positive".into())
        );
        assert_eq!(
            p(PromptKind::Interval, "0.5"),
            Err("non-root users cannot request an interval < 1.0 seconds".into())
        );
        assert_eq!(
            parse_prompt(PromptKind::Interval, "0.5", &cfg, true),
            Ok(UserAction::SetInterval(500))
        );
        assert_eq!(p(PromptKind::FirstTtl, "5"), Ok(UserAction::SetFirstTtl(5)));
        assert_eq!(
            p(PromptKind::FirstTtl, "31"),
            Err("firstTTL(31) cannot be larger than maxTTL(30).".into())
        );
        assert_eq!(
            p(PromptKind::FirstTtl, "0"),
            Err("first TTL must be at least 1".into())
        );
        assert_eq!(p(PromptKind::MaxTtl, "40"), Ok(UserAction::SetMaxTtl(40)));
        assert_eq!(
            p(PromptKind::MaxTtl, "0"),
            Err("maxTTL(0) cannot be less than firstTTL(1).".into())
        );
        assert_eq!(
            p(PromptKind::MaxTtl, "256"),
            Err("maxTTL(256) cannot be larger than 255.".into())
        );
        assert_eq!(
            p(PromptKind::Fields, "DR AGJMXI"),
            Ok(UserAction::SetFields("DR AGJMXI".into()))
        );
        assert_eq!(
            p(PromptKind::Fields, "LS "),
            Ok(UserAction::SetFields("LS ".into())),
            "trailing spacer kept"
        );
        assert_eq!(
            p(PromptKind::Fields, ""),
            Err("fields unchanged".into()),
            "curses.c:355: empty keeps"
        );
        assert_eq!(
            p(PromptKind::Fields, "LQ"),
            Err("Unknown field identifier: Q".into())
        );
        assert!(p(PromptKind::PacketSize, "abc").is_err());
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn toggle_status_texts() {
        let mut cfg = Config::default();
        cfg.dns = false;
        assert_eq!(
            toggle_status(&UserAction::ToggleDns, &cfg),
            Some("DNS off".into())
        );
        cfg.ipinfo_fields = vec![0];
        assert_eq!(
            toggle_status(&UserAction::ToggleAsn, &cfg),
            Some("ASN on".into())
        );
        cfg.mpls = true;
        assert_eq!(
            toggle_status(&UserAction::ToggleMpls, &cfg),
            Some("MPLS on".into())
        );
        cfg.protocol = mtr_proto::Protocol::Udp;
        assert_eq!(
            toggle_status(&UserAction::CycleProtocol, &cfg),
            Some("Protocol: UDP".into())
        );
        assert_eq!(
            toggle_status(&UserAction::Pause, &cfg),
            Some("Paused".into())
        );
        assert_eq!(
            toggle_status(&UserAction::Resume, &cfg),
            Some("Resumed".into())
        );
        assert_eq!(
            toggle_status(&UserAction::Reset, &cfg),
            Some("Statistics reset".into())
        );
        assert_eq!(toggle_status(&UserAction::SetTos(1), &cfg), None);
    }
}
