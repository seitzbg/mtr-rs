//! Codec for the text line protocol spoken between `mtr` and `mtr-packet`.
//! Reference: packet/cmdparse.c, packet/command.c, packet/probe.c and ui/cmdpipe.c
//! in mtr 0.96 (commit 7b01773). GPL-2.0-only.
#![forbid(unsafe_code)]

pub mod error;
pub mod tokenize;

pub use error::ParseError;

/// `MAX_COMMAND_ARGUMENTS` in packet/cmdparse.h.
pub const MAX_ARGUMENTS: usize = 16;
/// `COMMAND_BUFFER_SIZE` in packet/command.h: longest line the helper buffers, newline included.
pub const COMMAND_BUFFER_SIZE: usize = 4096;
/// `MAXLABELS` in ui/mtr.h: MPLS labels kept per reply.
pub const MAX_LABELS: usize = 8;
