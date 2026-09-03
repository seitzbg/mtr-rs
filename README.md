# mtr-rs

Rust port of [mtr](https://github.com/traviscross/mtr) (My TraceRoute). Two binaries as upstream:
an unprivileged `mtr` client and a privileged `mtr-packet` helper speaking the same line protocol
as the C helper, so either side interoperates with the C implementation. GPL-2.0-only.

Status: **Plans A and B complete** — the interactive TUI is the default; `mtr -r/-w/-j/-C` produce the
classic reports. Both drive the installed C `mtr-packet` (`$MTR_PACKET` overrides the search path).
The Rust `mtr-packet` (Plan C) comes next.

## Usage

    cargo build --release
    target/release/mtr example.org                    # interactive TUI (q quits, ? lists keys)
    target/release/mtr --ascii --no-color example.org # plain glyphs; NO_COLOR is honoured too
    target/release/mtr --report-on-exit example.org   # print the -r report when the TUI closes
    target/release/mtr -r -c 5 example.org            # classic report
    target/release/mtr -rwz -c 5 example.org          # wide report with AS numbers
    target/release/mtr -j example.org | jq .          # JSON, same schema as C mtr
    MTR_RS_LOG=/tmp/mtr.log target/release/mtr 1.1.1.1  # debug log (never written to the screen)

Keys follow C mtr (`p`/space, `r`, `n`, `z`, `e`, `s`, `b`, `i`, `f`, `m`, `o`, `Q`, `u`/`t`, `?`) plus
`↑`/`↓`/`j`/`k` to select a hop, `Enter` to toggle the detail pane, `Tab` to switch RTT / Addresses / Log,
and `d` to toggle the Recent sparkline column.

## Development

    cargo test --workspace                           # unit, scenario and fake-helper tests
    INSTA_UPDATE=always cargo test -p mtr --test tui_snapshots   # accept intended screen changes
    MTR_E2E=1 cargo test -p mtr -- --ignored         # real DNS and the installed C helper

Design: `docs/superpowers/specs/`. Plans: `docs/superpowers/plans/`.
