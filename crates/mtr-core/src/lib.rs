//! Deterministic probing engine: scheduling, per-hop statistics, ECMP tracking.
//! Ported from ui/net.c and ui/select.c (mtr 0.96, commit 7b01773). GPL-2.0-only.
#![forbid(unsafe_code)]
