//! The interactive event loop: `select_loop()` of ui/select.c (mtr 0.96, commit 7b01773) with a
//! keyboard stream and a coalesced renderer (spec §8.2). GPL-2.0-only.

use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyEventKind};
use futures::{Stream, StreamExt as _};
use ratatui::Terminal;
use ratatui::backend::Backend;

use crate::driver::{Driver, Wake};
use crate::tui::glyphs::Glyphs;
use crate::tui::input::{Input, map_key, parse_prompt, toggle_status};
use crate::tui::palette::Palette;
use crate::tui::render::{View, bounds, draw};
use crate::tui::state::{Quit, UiAction, UiState};

pub const FRAME_MIN_GAP: Duration = Duration::from_millis(50);
pub const CLOCK_TICK: Duration = Duration::from_millis(250);

pub struct TuiOptions<'a> {
    pub glyphs: &'static Glyphs,
    pub palette: Palette,
    pub is_root: bool,
    pub local_hostname: &'a str,
    pub target_name: &'a str,
}

pub struct TuiOutcome {
    /// Ctrl-C ended the run (exit code 130, as C).
    pub interrupted: bool,
}

pub fn clock_text(now: &jiff::Zoned) -> String {
    now.strftime("%H:%M:%S").to_string()
}

/// The whole screen as a `Rect` (ratatui 0.30's `Terminal::size()` returns a `Size`).
fn screen<B: Backend>(terminal: &Terminal<B>) -> Result<ratatui::layout::Rect, B::Error> {
    let s = terminal.size()?;
    Ok(ratatui::layout::Rect::new(0, 0, s.width, s.height))
}

pub async fn run<B, S>(
    terminal: &mut Terminal<B>,
    driver: &mut Driver<'_>,
    mut events: S,
    opts: &TuiOptions<'_>,
) -> anyhow::Result<TuiOutcome>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
    S: Stream<Item = std::io::Result<Event>> + Unpin,
{
    let mut ui = UiState::new();
    let mut dirty = true;
    let mut last_frame = Instant::now() - FRAME_MIN_GAP;
    let mut clock = tokio::time::interval(CLOCK_TICK);
    let mut ctrl_c = std::pin::pin!(tokio::signal::ctrl_c());
    let version = env!("CARGO_PKG_VERSION");
    loop {
        let now = Instant::now();
        if dirty && now.duration_since(last_frame) >= FRAME_MIN_GAP {
            let area = screen(terminal)?;
            ui.clamp(&bounds(area, driver.engine, &ui));
            if ui.redraw {
                terminal.clear()?;
                ui.redraw = false;
            }
            let clock_s = clock_text(&jiff::Zoned::now());
            let view = View {
                engine: driver.engine,
                names: driver.names,
                ui: &ui,
                glyphs: opts.glyphs,
                palette: &opts.palette,
                now,
                clock: &clock_s,
                local_hostname: opts.local_hostname,
                target_name: opts.target_name,
                version,
            };
            terminal.draw(|f| draw(f, &view))?;
            dirty = false;
            last_frame = Instant::now();
        }
        let render_due = async {
            if dirty {
                tokio::time::sleep_until((last_frame + FRAME_MIN_GAP).into()).await
            } else {
                std::future::pending::<()>().await
            }
        };
        tokio::select! {
            w = driver.wait_wake() => {
                if driver.step(w).await?.finished {
                    return Ok(TuiOutcome { interrupted: false });
                }
                dirty = true;
            }
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k))) if k.kind != KeyEventKind::Release => {
                    let b = bounds(screen(terminal)?, driver.engine, &ui);
                    let now = Instant::now();
                    match map_key(k, &ui) {
                        Input::Engine(a) => {
                            driver.step(Wake::Action(a.clone())).await?;
                            if let Some(s) = toggle_status(&a, driver.engine.config()) {
                                ui.set_status(s, now);
                            }
                        }
                        Input::Ui(a) => {
                            if let Some(sub) = ui.apply(a, &b, now) {
                                match parse_prompt(sub.kind, &sub.text, driver.engine.config(), opts.is_root) {
                                    Ok(a) => { driver.step(Wake::Action(a)).await?; }
                                    Err(msg) => ui.set_status(msg, now),
                                }
                            }
                        }
                        Input::None => {}
                    }
                    dirty = true;
                }
                Some(Ok(Event::Resize(_, _))) => { ui.redraw = true; dirty = true; }
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(anyhow::Error::new(e).context("terminal input")),
                None => {
                    // EOF on the key stream = C's `getch() == -1` → ActionQuit
                    let b = bounds(screen(terminal)?, driver.engine, &ui);
                    ui.apply(UiAction::Quit(Quit::Key), &b, Instant::now());
                }
            },
            _ = clock.tick() => { ui.expire_status(Instant::now()); dirty = true; }
            _ = render_due => {}
            _ = &mut ctrl_c => return Ok(TuiOutcome { interrupted: true }),
        }
        if let Some(q) = ui.quit {
            return Ok(TuiOutcome {
                interrupted: q == Quit::CtrlC,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn clock_is_hh_mm_ss() {
        let z: jiff::Zoned = "2026-09-03T12:34:56[UTC]".parse().unwrap();
        assert_eq!(super::clock_text(&z), "12:34:56");
    }
}
