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
