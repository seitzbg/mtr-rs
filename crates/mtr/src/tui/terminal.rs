//! Raw-mode / alternate-screen guard and the panic hook that restores the terminal (spec §8, §9).
//! GPL-2.0-only.

use std::io::{self, Write as _};

use crossterm::cursor::Show;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

/// Restores the terminal on drop. Only one should exist at a time.
pub struct Guard;

pub fn enter() -> io::Result<Guard> {
    enable_raw_mode()?;
    if let Err(e) = crossterm::execute!(io::stdout(), EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(e);
    }
    Ok(Guard)
}

/// Best-effort restore; safe to call twice (crossterm ignores an already-cooked terminal).
///
/// `Terminal::draw` hides the cursor on every frame that does not place one (a prompt does), and
/// DECTCEM is not part of the `?1049` save/restore set, so the cursor must be shown explicitly or
/// the shell prompt comes back invisible.
pub fn restore() {
    let _ = crossterm::execute!(io::stdout(), Show, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    let _ = io::stdout().flush();
}

impl Drop for Guard {
    fn drop(&mut self) {
        restore();
    }
}

/// Wrap the current panic hook so the message is printed on a restored screen.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
}
