//! Non-interactive output over the engine state — ui/report.c (mtr 0.96, commit 7b01773). GPL-2.0-only.

pub mod csv;
pub mod json;
pub mod report;

use mtr_core::Engine;
use mtr_core::fields::Field;

use crate::names::NameCache;

/// Everything the emitters need besides `engine.config()`.
pub struct ReportContext<'a> {
    pub engine: &'a Engine,
    pub names: &'a NameCache,
    /// `LocalHostname` (gethostname()).
    pub local_hostname: &'a str,
    /// `ctl->Hostname`: the target exactly as given on the command line.
    pub target_name: &'a str,
    /// `-w`.
    pub wide: bool,
    /// The active fields from `-o`, in order (spacer included).
    pub fields: Vec<&'static Field>,
}

/// What `--report-on-exit` prints once the TUI has closed and the terminal is restored: the plain
/// `-r` report (wide when `ctx.wide`), without the `Start:` line — `display.c:145-152` calls
/// `report_close()` only. Empty when the flag is off.
pub fn report_on_exit_text(ctx: &ReportContext<'_>, report_on_exit: bool) -> String {
    if report_on_exit {
        report::render(ctx)
    } else {
        String::new()
    }
}
