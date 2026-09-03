# mtr-rs

Rust port of [mtr](https://github.com/traviscross/mtr) (My TraceRoute). Two binaries as upstream:
an unprivileged `mtr` client and a privileged `mtr-packet` helper speaking the same line protocol
as the C helper, so either side interoperates with the C implementation. GPL-2.0-only.

Status: **Plan A complete** — `mtr -r/-w/-j/-C` work end to end, driving the installed C `mtr-packet`
(`$MTR_PACKET` overrides the search path). The TUI (Plan B) and the Rust `mtr-packet` (Plan C) come next.

## Usage

    cargo build --release
    target/release/mtr -r -c 5 example.org           # classic report
    target/release/mtr -rwz -c 5 example.org         # wide report with AS numbers
    target/release/mtr -j example.org | jq .          # JSON, same schema as C mtr
    MTR_RS_LOG=/tmp/mtr.log target/release/mtr -r 1.1.1.1   # debug log

Interactive mode is not implemented yet; use one of `-r`, `-w`, `-j`, `-C`.

## Development

    cargo test --workspace                           # unit, scenario and fake-helper tests
    MTR_E2E=1 cargo test -p mtr -- --ignored         # real DNS and the installed C helper

Design: `docs/superpowers/specs/`. Plans: `docs/superpowers/plans/`.
