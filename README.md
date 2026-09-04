# mtr-rs

Rust port of [mtr](https://github.com/traviscross/mtr) (My TraceRoute). Two binaries as upstream:
an unprivileged `mtr` client and a privileged `mtr-packet` helper speaking the same line protocol
as the C helper, so either side interoperates with the C implementation. GPL-2.0-only.

Status: **Plans A, B and C complete** — the interactive TUI is the default, `mtr -r/-w/-j/-C` produce
the classic reports, and the Rust `mtr-packet` helper ships alongside them. Either helper works:
`$MTR_PACKET` overrides the search path, so the client can drive our helper or the installed C one,
and the C client can drive ours.

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

## Helper (`mtr-packet`)

`mtr-packet` is the privileged half of mtr: it opens the raw sockets, sends the probes and reports
replies, while the `mtr` client stays unprivileged and only speaks a line protocol to it over
stdin/stdout. Our helper is **wire-compatible** with the C `mtr-packet` of mtr 0.96, so the C client
drives ours and our client drives the C one — the two halves are interchangeable.

The client looks for a helper in the C search order: `$MTR_PACKET` (ignored when
`/etc/mtr.is.run.under.sudo` exists), `mtr-packet` on `PATH`, `mtr-packet` next to the running
executable, then `./mtr-packet`. When a helper reports `permission-denied` the client prints the
install hint `sudo setcap cap_net_raw+ep "$(command -v mtr-packet)"`.

### Privileges

    cargo build --workspace
    sudo setcap cap_net_raw+ep target/debug/mtr-packet

The helper opens every socket it will ever need at startup and then **drops everything** before it
reads the first byte of stdin: `setgid(getgid())`, `setuid(getuid())`, a check that the effective ids
really changed, and only then the effective, permitted and inheritable capability sets are cleared
(the order of `drop_elevated_permissions()` in the C `packet/packet.c`). From that point it is an
ordinary unprivileged process serving requests on the sockets it already holds.

Without `cap_net_raw` (and without root) the raw sockets fail to open and the helper falls back to
unprivileged `SOCK_DGRAM` ICMP/UDP sockets with `IP_RECVERR`/`IPV6_RECVERR`, reading ICMP errors off
the socket error queue. That path needs the kernel's ping sockets to be open to your group
(`/proc/sys/net/ipv4/ping_group_range`); it answers ICMP and UDP probes, and `check-support` reports
what actually opened. TCP and SCTP probes reach their destination without privilege — the
`connect()` they are built on needs none — but the `ttl-expired` answer from an *intermediate* hop
arrives as an ICMP time-exceeded message, which only the raw ICMP receive socket can read. So
unprivileged TCP/SCTP probes see the final hop's `reply` (or `no-reply`), not the path.

### Compat suites

The upstream Python suites from the C repo run unmodified against our helper:

    export MTR_C_REPO=~/git/mtr                        # default; the C checkout at 7b01773
    MTR_PACKET=$PWD/target/debug/mtr-packet tests/compat/run.sh cmdparse TestCommandParse
    MTR_PACKET=$PWD/target/debug/mtr-packet tests/compat/run.sh --compare       # ours vs the C helper
    MTR_PACKET=$PWD/target/debug/mtr-packet tests/compat/run.sh --report-only probe   # never fails

`--compare` runs each suite against our helper and against the baseline C helper
(`${MTR_BASELINE:-/usr/bin/mtr-packet}`), diffs the failing sets **by unittest test id** and fails
only on failures that are ours alone — environmental failures (no global IPv6, no network) cancel
out. Ids on the script's known-divergence list are printed as `known: <id> -- <reason>` and never
fail the run. `--report-only` prints the same summary and always exits 0, for runs without a
baseline. `param.py` and `probe.py` need capabilities:

    sudo setcap cap_net_raw+ep target/debug/mtr-packet
    sudo setcap cap_net_raw+ep "$MTR_C_REPO"/test/mtr-packet-listen   # built on demand by run.sh

`param.py` skips itself with a message when the listener lacks `cap_net_raw`. Its four tests are on
the known-divergence list *while the listener is the stock one*: upstream's `test/packet_listen.c`
still hard-codes `SEQUENCE_NUM 33000`, but mtr commit e95eaf4 moved `MIN_PORT` to 33434 in
`packet/probe_unix.h`, so a 0.96-conformant helper (ours, and the C helper at 7b01773) makes the
listener time out. To verify `param.py` positively, rebuild the listener for the real first
sequence — the wrapper patches a scratch copy of the source under `target/compat/` rather than
touching the C repo:

    tests/compat/run.sh --listen-seq 33434 param -v
    sudo setcap cap_net_raw+ep "$MTR_C_REPO"/test/mtr-packet-listen   # the rebuild drops the cap

Once the listener matches `MIN_PORT`, those four ids come **off** the known-divergence list and
gate again, so a real size/TOS/bit-pattern regression fails `--compare`. `--listen-seq` is what
rebuilds; `MTR_LISTEN_SEQUENCE=33434` on its own only declares which sequence an already-built,
already-`setcap`'d listener was compiled for (that is what CI does, so it does not strip the cap).

In `cargo test` only
`cmdparse.py TestCommandParse` runs (it needs no privileges); `param.py` and `probe.py` are behind
`MTR_E2E=1 cargo test -p mtr-packet -- --ignored`.

## Development

    cargo test --workspace                           # unit, scenario and fake-helper tests
    INSTA_UPDATE=always cargo test -p mtr --test tui_snapshots   # accept intended screen changes
    MTR_E2E=1 cargo test -p mtr -- --ignored         # real DNS and the installed C helper

Design: `docs/superpowers/specs/`. Plans: `docs/superpowers/plans/`.
