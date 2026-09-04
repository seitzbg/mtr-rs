# mtr-rs

Rust port of [mtr](https://github.com/traviscross/mtr) (My TraceRoute). Two binaries as upstream:
an unprivileged `mtr` client and a privileged `mtr-packet` helper speaking the same line protocol
as the C helper, so either side interoperates with the C implementation. GPL-2.0-only.

Status: **Plans A–D complete** — the interactive TUI is the default, `mtr -r/-w/-j/-C` produce
the classic reports, and the Rust `mtr-packet` helper ships alongside them. Either helper works:
`$MTR_PACKET` overrides the search path, so the client can drive our helper or the installed C one,
and the C client can drive ours. Packaged: release tarballs and a Debian package, generated man
pages and completions, and scripts/install.sh.

## Installation

From a release. The tarball unpacks to a single `mtr-rs-<version>-<arch>/` directory holding
`bin/`, `man/`, `completions/` and the `LICENSE` and `README.md`:

    tar xzf mtr-rs-0.1.0-x86_64.tar.gz
    cd mtr-rs-0.1.0-x86_64
    sudo install -m 755 bin/mtr bin/mtr-packet /usr/local/bin/
    sudo install -m 644 man/*.8 /usr/local/share/man/man8/
    sudo setcap cap_net_raw+ep /usr/local/bin/mtr-packet

On Debian and Ubuntu the `.deb` installs the same files and runs `setcap` from its `postinst`:

    sudo dpkg -i mtr-rs_0.1.0-1_amd64.deb

The package is named `mtr-rs` and declares `Conflicts: mtr, mtr-tiny`, because it owns
`/usr/bin/mtr` and `/usr/bin/mtr-packet`. Installing it therefore removes the distribution's
`mtr` or `mtr-tiny` package — that conflict is by design, not a packaging accident.

From a checkout, `scripts/install.sh` installs both binaries with the `mtr(8)` and `mtr-packet(8)`
man pages and the bash, zsh and fish completions, and then runs `setcap`. The default prefix is
`/usr/local`, which needs root — so build the artefacts as yourself and copy them as root:

    cargo build --release --workspace && cargo xtask dist
    sudo scripts/install.sh --no-build

`--no-build` compiles nothing and never invokes cargo, so nothing is built as root; it fails with
the `cargo xtask dist` command to run if the man pages or completions are missing. For a
single-user install no root is needed at all, and the script builds for you:

    scripts/install.sh --prefix ~/.local

    scripts/install.sh --uninstall --prefix ~/.local   # --no-setcap and --help also exist

`--uninstall` removes exactly the files it installed (pass the same `--prefix`). If `setcap` is missing or fails, the script prints the exact command to run and still
exits 0: `mtr-packet` keeps working on its unprivileged ICMP-DGRAM fallback, which requires your
gid to be inside `net.ipv4.ping_group_range`.

Binaries only, without man pages or completions:

    cargo install --path crates/mtr
    cargo install --path crates/mtr-packet
    sudo setcap cap_net_raw+ep "$(command -v mtr-packet)"

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

Every key is optional, but the file is strict: an unknown key (or an unknown `color` value) is an
error, so a typo is reported rather than silently ignored. An absent file is normal; an unreadable
or malformed one is a fatal `mtr: config: <path>: <error>`. Under sudo `--config` is refused and
the default path is not read, the same way `-F` is disabled. `docs/config.example.toml` is the
same content `--init-config` writes. For a single run, `--rtt-thresholds 30,100,200,500` sets the
colour ramp and `--color auto|always|never` overrides `display.color`.

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

The one exception is `CAP_NET_ADMIN`, which the drop keeps — and keeps *only* when the process held it
going in, in the effective and permitted sets, never in the inheritable one — because `SO_MARK` is set
per probe, after the drop. That covers a helper given the capability with `setcap` and a helper run as
root, which holds it by uid; in both cases everything else, `cap_net_raw` included, still goes. So
`-M`/`--mark` needs `cap_net_admin` on the helper, and the client's own route lookup (a `connect()` on
a marked UDP socket) needs it too, which in practice means running `mtr` as root. (`CAP_NET_ADMIN` is
what the kernel checks for `SO_MARK` here; since Linux 5.17 `cap_net_raw` also unlocks it, but the drop
removes that one. Inside a user namespace whose network namespace belongs to a different user
namespace, the capability is not enough and `setsockopt` still fails.) Grant both capabilities to the
helper with:

    sudo setcap cap_net_raw,cap_net_admin+ep "$(command -v mtr-packet)"

`check-support feature mark` answers `ok` only when the helper really holds `CAP_NET_ADMIN`; C always
says `ok` and then fails in `setsockopt()`. The packaging (`packaging/debian/postinst`,
`scripts/install.sh`) grants `cap_net_raw` only, so `--mark` is opt-in.

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

    cargo xtask man                                  # target/dist/man/{mtr.8,mtr-packet.8}
    cargo xtask completions                          # target/dist/completions/{mtr.bash,_mtr,mtr.fish}
    cargo xtask dist                                 # both, plus a release build, laid out as
                                                     # target/dist/mtr-rs-<version>-<arch>/{bin,man,completions}

### Releasing

1. Bump `version` under `[workspace.package]` in the root `Cargo.toml`, run `cargo check --workspace`
   so `Cargo.lock` follows, and commit.
2. `git tag v0.1.0 && git push --tags`. The tag must be the workspace version with a leading `v`;
   `.github/workflows/release.yml` compares them and fails the job otherwise.
3. The `release` workflow builds x86_64 and aarch64, runs `scripts/check-deb.sh`, and attaches
   `mtr-rs-<version>-<arch>.tar.gz` and `mtr-rs_<version>-1_<debarch>.deb` to the GitHub release
   for the tag. A `workflow_dispatch` run with `dry_run: "true"` builds the same artifacts, uploads
   them as workflow artifacts, and creates no release.

