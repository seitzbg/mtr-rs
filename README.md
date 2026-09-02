# mtr-rs

Rust port of [mtr](https://github.com/traviscross/mtr) (My TraceRoute). Two binaries as upstream:
an unprivileged `mtr` client and a privileged `mtr-packet` helper speaking the same line protocol
as the C helper, so either side interoperates with the C implementation. GPL-2.0-only.

Status: **Plan A** — `-r/-w/-j/-C` report modes driving the installed C `mtr-packet`.
The TUI (Plan B) and the Rust helper (Plan C) come next.

    cargo build --release
    target/release/mtr -r -c 5 example.org        # uses /usr/bin/mtr-packet or $MTR_PACKET

Design: `docs/superpowers/specs/`. Plans: `docs/superpowers/plans/`.
