//! mtr-packet: the privileged probe helper of mtr-rs. Rust port of mtr 0.96's `packet/`
//! (commit 7b01773). GPL-2.0-only.
#![forbid(unsafe_code)]

pub mod backend;
pub mod command;
pub mod probe_table;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, thiserror::Error)]
pub enum Fatal {
    #[error("{0}")]
    Message(String),
    #[error("{0}: {1}")]
    Io(String, std::io::Error),
}

pub fn run() -> Result<(), Fatal> {
    Err(Fatal::Message("not implemented".into()))
}
