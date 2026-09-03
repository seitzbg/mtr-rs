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

## Configuration

Optional, and entirely a Rust-port addition: `~/.config/mtr-rs/config.toml`
(`$XDG_CONFIG_HOME/mtr-rs/config.toml` when that variable is set, `--config PATH` to point
elsewhere). Write a fully commented starter file with

    mtr --init-config      # creates the file, prints its path, never overwrites one

The file only supplies defaults. Precedence, lowest to highest:

    built-in defaults  <  config file  <  $MTR_OPTIONS  <  the command line

    [display]
    rtt_thresholds_ms = [30, 100, 200, 500]  # RTT colour ramp: green|yellow|magenta|red|bold red
    fields = "LS NABWV"                      # the field letters of -o
    ascii = false                            # --ascii
    color = "auto"                           # auto | always | never ("never" is --no-color)
    sparkline = true                         # Recent column shown when the TUI starts
    detail_pane = true                       # detail pane open when the TUI starts

    [probe]
    interval = 1.0                           # -i
    max_ttl = 30                             # -m
    max_unknown = 12                         # -U
    timeout = 10                             # -Z
    dns = true                               # false is -n
    asn = false                              # true is -z

Every key is optional. An absent file is normal; an unreadable or malformed one is a fatal
`mtr: config: <path>: <error>`. `docs/config.example.toml` is the same content `--init-config`
writes. `--rtt-thresholds 30,100,200,500` sets the colour ramp for a single run.

## Development

    cargo test --workspace                           # unit, scenario and fake-helper tests
    INSTA_UPDATE=always cargo test -p mtr --test tui_snapshots   # accept intended screen changes
    MTR_E2E=1 cargo test -p mtr -- --ignored         # real DNS and the installed C helper

Design: `docs/superpowers/specs/`. Plans: `docs/superpowers/plans/`.
