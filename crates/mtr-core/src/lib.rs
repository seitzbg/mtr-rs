//! Deterministic probing engine: scheduling, per-hop statistics, ECMP tracking.
//! Ported from ui/net.c and ui/select.c (mtr 0.96, commit 7b01773). GPL-2.0-only.
#![forbid(unsafe_code)]

pub mod config;
pub mod fields;
pub mod history;
pub mod hop;
pub mod rng;
pub mod stats;

pub use config::Config;
pub use fields::{FIELDS, Field, FieldFormat};
pub use history::{History, Sample};
pub use hop::{Hop, HopAddr, HopError, Reply};
pub use rng::Rng;
pub use stats::RttStats;

/// `MaxHost` (ui/mtr.h:75): hop slots, indexed by ttl - 1.
pub const MAX_HOST: usize = 256;
/// `MAX_PATH` (ui/mtr.h:74): distinct addresses remembered per hop.
pub const MAX_PATH: usize = 128;
/// `MINPACKET` / `MAXPACKET` (ui/mtr.h:78-79).
pub const MIN_PACKET: i32 = 28;
pub const MAX_PACKET: i32 = 65535;
/// `MinSequence` / `MaxSequence` (ui/net.c:49-50): probe tokens live in `[33000, 65536)`.
pub const MIN_SEQUENCE: u32 = 33000;
pub const MAX_SEQUENCE: u32 = 65536;
/// Initial `numhosts` (ui/net.c:126): pacing divisor before the first batch completes.
pub const DEFAULT_NUMHOSTS: u32 = 10;
/// Probes remembered per hop for sparklines/charts (C keeps `SAVED_PINGS` = 400).
pub const HISTORY_LEN: usize = 1024;
