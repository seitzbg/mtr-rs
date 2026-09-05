# mtr-rs

A Rust port of [mtr](https://github.com/traviscross/mtr) (My TraceRoute). Like upstream it is two
binaries: the unprivileged `mtr-rs` client and the privileged `mtr-rs-packet` helper. Both speak
the C mtr 0.96 line protocol, so each half also works with its C counterpart. The TUI is the
default; `-r`, `-w`, `-j` and `-C` give the classic reports. Linux and macOS on x86_64 and
aarch64, FreeBSD on x86_64. GPL-2.0-only.

## Screenshots

![mtr-rs interactive view](docs/screenshots/tui.svg)

![Detail pane](docs/screenshots/tui-detail.svg)

## Install

Every release ships `mtr-rs-<version>-<arch>-<os>.tar.gz` tarballs (Linux, macOS, FreeBSD), `.deb`s
and a FreeBSD `.pkg`. The one platform-specific step is giving `mtr-rs-packet` the right to open raw
sockets: a capability on Linux, setuid root on macOS and FreeBSD (it drops back to you once its
sockets are open).

    # macOS: Homebrew (Apple silicon and Intel; also works with Linuxbrew)
    brew install seitzbg/mtr-rs/mtr-rs                 # Homebrew 6 may first ask for: brew trust seitzbg/mtr-rs
    sudo chown root:wheel "$(brew --prefix)/opt/mtr-rs/bin/mtr-rs-packet"   # repeat after upgrades
    sudo chmod u+s "$(brew --prefix)/opt/mtr-rs/bin/mtr-rs-packet"

    # macOS: tarball (unsigned; clear the quarantine flag Gatekeeper adds to downloads)
    tar xzf mtr-rs-0.3.0-aarch64-macos.tar.gz && cd mtr-rs-0.3.0-aarch64-macos
    xattr -d com.apple.quarantine bin/* 2>/dev/null
    sudo install -m 755 bin/mtr-rs bin/mtr-rs-packet /usr/local/bin/
    sudo install -m 644 man/*.8 /usr/local/share/man/man8/
    sudo chown root:wheel /usr/local/bin/mtr-rs-packet && sudo chmod u+s /usr/local/bin/mtr-rs-packet

    # Linux: tarball
    tar xzf mtr-rs-0.3.0-x86_64-linux.tar.gz && cd mtr-rs-0.3.0-x86_64-linux
    sudo install -m 755 bin/mtr-rs bin/mtr-rs-packet /usr/local/bin/
    sudo install -m 644 man/*.8 /usr/local/share/man/man8/
    sudo setcap cap_net_raw+ep /usr/local/bin/mtr-rs-packet

    # Linux: Debian package, setcap from its postinst
    sudo dpkg -i mtr-rs_0.3.0-1_amd64.deb

    # FreeBSD: package, mtr-rs-packet setuid root (or the tarball, as for macOS)
    sudo pkg add mtr-rs-0.3.0-x86_64-freebsd.pkg

    # from a checkout, with man pages and bash/zsh/fish completions
    cargo build --release --workspace && cargo xtask dist
    sudo scripts/install.sh --no-build                # /usr/local needs root; never runs cargo
    scripts/install.sh --prefix ~/.local              # single-user: no root, builds for you
    scripts/install.sh --uninstall --prefix ~/.local  # --no-setcap, --help also exist

    # binaries only, no man pages or completions
    cargo install --path crates/mtr
    cargo install --path crates/mtr-packet
    sudo setcap cap_net_raw+ep "$(command -v mtr-rs-packet)"

Neither package declares a conflict with the distribution's `mtr`, so it can stay installed.
`--uninstall` removes exactly what it installed, given the same `--prefix`. A failing privilege
grant (`setcap`, or the BSD `chmod u+s` without root) prints the command and exits 0.
`scripts/install.sh` needs bash, which on FreeBSD is `pkg install bash`; macOS ships it.

## Usage

    mtr-rs example.org                      # interactive TUI (q quits, ? for keys)
    mtr-rs --ascii example.org              # plain glyphs (NO_COLOR honoured too)
    mtr-rs --report-on-exit example.org     # -r report when the TUI closes
    mtr-rs -r -c 5 example.org              # classic report
    mtr-rs -rwz -c 5 example.org            # wide report with AS numbers
    mtr-rs -j example.org | jq .            # C mtr's schema (array for several targets)
    MTR_RS_LOG=/tmp/mtr.log mtr-rs 1.1.1.1  # debug log (new file only, off under sudo)

Keys follow C mtr (`p`/space, `r`, `n`, `z`, `e`, `s`, `b`, `i`, `f`, `m`, `o`, `Q`, `u`/`t`, `?`);
`↑`/`↓`/`j`/`k` select a hop, `Enter` opens the detail pane, `Tab` cycles its RTT, Addresses and Log
tabs, `d` toggles the Recent sparkline.

## Checking the install

Run these once after installing, as your normal user (not root). Each one should print a hop list
ending in the target; a run that only ever prints `???` for every hop, or exits with a privilege
hint, means the helper cannot open its sockets, and the hint names the fix for your OS.

    mtr-rs --version                         # client version
    mtr-rs -r -c 3 127.0.0.1                 # ICMP to loopback: one hop, ~0 ms, no privileges beyond the helper's
    mtr-rs -r -c 3 1.1.1.1                   # ICMP over the real path: several hops, the last one 1.1.1.1
    mtr-rs -6 -r -c 3 2606:4700:4700::1111   # the same over IPv6 (needs IPv6 connectivity)
    mtr-rs -u -r -c 3 1.1.1.1                # UDP probes: same hops, final hop answers with port-unreachable
    mtr-rs -T -P 443 -r -c 3 1.1.1.1         # TCP SYN probes to a port that is open on the target
    mtr-rs -T -P 9 -r -c 3 1.1.1.1           # TCP to a closed port: still reaches the last hop (RST counts as a reply)
    mtr-rs -I en0 -r -c 3 1.1.1.1            # source interface (eth0 on Linux); fails cleanly for a bad name
    mtr-rs -e -r -c 3 1.1.1.1                # MPLS labels, shown only where a router attaches them
    mtr-rs -rwz -c 3 1.1.1.1                 # wide report with AS numbers (DNS TXT lookups)
    mtr-rs -j -c 3 1.1.1.1 | jq .            # JSON, same schema as C mtr
    mtr-rs 1.1.1.1                           # the TUI; q quits, ? lists the keys

What differs by platform:

- Linux without `cap_net_raw` on the helper: ICMP and UDP still work through unprivileged sockets,
  TCP and SCTP see the final hop only, MPLS labels are not decoded. With the capability all modes
  work. `-M`/`--mark` needs `cap_net_admin` as well (see below).
- macOS and FreeBSD: the helper must be setuid root, or everything above has to run under `sudo`.
  `-M` is refused, and SCTP is reported as unsupported (macOS has no SCTP; FreeBSD needs the
  `sctp` kernel module). On macOS a UDP run with both `-P` and `-L` fixed misses the final hop when
  the destination is itself a Mac (see Differences from C mtr).
- Intermediate hops that show `???` on every run are routers that rate-limit or drop ICMP
  time-exceeded; C mtr shows the same. Compare with `mtr` or `traceroute` if in doubt.

When something looks wrong, capture the helper conversation and open an issue with it:

    MTR_RS_LOG=/tmp/mtr-rs.log mtr-rs -r -c 3 1.1.1.1   # the log holds every command and reply
    printf '1 check-support feature ip-4\n2 check-support feature mark\n' | mtr-rs-packet
                                                        # talk to the helper directly: "support ok"/"support no"

## Configuration

The config file is optional. Write a commented starter file:

    mtr-rs --init-config          # prints the path, never overwrites
    mtr-rs --init-config -i 2 -z  # the same, with those options set

It writes the options in effect (defaults, `$MTR_OPTIONS`, command line); keys still at their
default stay commented out. `docs/config.example.toml` is that file with nothing set and documents
every key. The client reads `~/.config/mtr-rs/config.toml` (or `$XDG_CONFIG_HOME/mtr-rs/config.toml`,
or `--config PATH`) as defaults. Precedence, lowest to highest:

    built-in defaults  <  config file  <  $MTR_OPTIONS  <  the command line

An unknown key is an error; an unreadable or malformed file is fatal. For one run,
`--rtt-thresholds 30,100,200,500` and `--color auto|always|never` override the file.

## Helper and privileges

The client talks to the helper over a pipe. It tries `$MTR_PACKET`, `mtr-rs-packet` on `PATH`,
beside the running executable, `./mtr-rs-packet`, then the C `mtr-packet`.

    sudo setcap cap_net_raw+ep "$(command -v mtr-rs-packet)"
    sudo sysctl -w net.ipv4.ping_group_range="0 2147483647"   # or open ping sockets to your gid

Without `cap_net_raw` the helper falls back to unprivileged ICMP and UDP datagram sockets, which
the kernel only hands to gids inside `net.ipv4.ping_group_range`; the client names both fixes when
it cannot open its sockets. On that path TCP and SCTP probes see the final hop only: time-exceeded
replies need a raw socket.

FreeBSD and macOS have neither capabilities nor an `IP_RECVERR` fallback, so there the helper is
installed setuid root, as the ports and Homebrew `mtr` are, and drops to the invoking user once its
raw sockets are open:

    sudo chown root:wheel "$(command -v mtr-rs-packet)" && sudo chmod 4755 "$(command -v mtr-rs-packet)"

`-M`/`--mark` (`SO_MARK`) is Linux only and the client refuses it elsewhere. The helper's
`local-device` is `SO_BINDTODEVICE` on Linux and `IP_BOUND_IF` on macOS, and unsupported on FreeBSD;
`-I` works everywhere because the client resolves the interface to a source address itself.
One macOS blind spot, shared with C mtr: a UDP probe with both `-P` and `-L` fixed carries its
sequence in the UDP checksum field, and Darwin zeroes that field in the port-unreachable it
quotes, so the final hop is unmatched when the destination is itself a Mac.

The helper opens its sockets, then drops setgid, setuid and its capabilities before reading stdin.
One exception: `CAP_NET_ADMIN` is kept when the helper started with it, because `SO_MARK` is set
per probe after the drop. `-M`/`--mark` therefore needs `cap_net_admin` on the helper, and root
(or the same capability) for the client's marked route lookup. The package and `scripts/install.sh`
grant `cap_net_raw` alone, so `--mark` is opt-in:

    sudo setcap cap_net_raw,cap_net_admin+ep "$(command -v mtr-rs-packet)"

When `/etc/mtr.is.run.under.sudo` exists the client ignores `$MTR_PACKET` and `$MTR_RS_LOG`, refuses
`-F`, `--config` and `--init-config`, does not read the default config file, and searches for the
helper only by absolute paths beside the client or in the standard `/usr/local` and `/usr`
`bin`/`sbin` directories (never via `PATH` or the current directory).

## Differences from C mtr

Deliberate differences, each with a code comment citing the C source:

- `-j` with several targets prints one JSON array; C concatenates objects into invalid JSON.
- CSV output quotes fields containing commas, quotes or newlines (RFC 4180); C never quotes. It is
  machine-readable CSV, not spreadsheet-sanitized output, so formula-leading values are preserved.
- The helper uses the IPv4 IHL field to locate headers; C assumes 20 bytes and misparses IP options.
- Only `64:ff9b::/96` counts as the well-known NAT64 prefix; C compares just 32 bits.
- `mark` support is reported only when the helper holds `CAP_NET_ADMIN`; C claims it whenever
  `SO_MARK` was compiled in.
- `-U` below 1 and a negative `-c` are range errors; C clamps the first and accepts the second.

## Development

    cargo test --workspace                                      # unit and scenario tests
    INSTA_UPDATE=always cargo test -p mtr --test tui_snapshots   # accept screen changes
    MTR_E2E=1 cargo test -p mtr -- --ignored                     # real DNS, the installed C helper
    cargo deny check                                             # advisories, licences, sources

    cargo xtask man          # target/dist/man/
    cargo xtask completions  # target/dist/completions/ (bash, zsh, fish)
    cargo xtask dist         # both, plus a release build, under target/dist/
    scripts/homebrew-formula.sh 0.3.0   # the Homebrew formula for a published release (seitzbg/homebrew-mtr-rs)

The upstream Python suites run unmodified against our helper. `--compare` fails only on failures
that are ours alone; `param.py` and `probe.py` want `cap_net_raw` on the C repo's listener too, and
an IPv6-capable host.

    export MTR_C_REPO=~/git/mtr MTR_PACKET=$PWD/target/debug/mtr-rs-packet
    tests/compat/run.sh cmdparse TestCommandParse   # no privileges, also runs in cargo test
    tests/compat/run.sh --compare                   # every suite vs $MTR_BASELINE
    tests/compat/run.sh --report-only probe         # same summary, always exits 0

User-visible changes go in `CHANGELOG.md` in the same commit; plans in `ROADMAP.md`.

## Releasing

1. Bump `version` under `[workspace.package]` in the root `Cargo.toml` and the two pins under
   `[workspace.dependencies]` (cargo-deny bans wildcards), run `cargo check --workspace` so
   `Cargo.lock` follows, and commit.
2. `git tag v<version> && git push --tags`; the job fails unless the tag is that version with a
   leading `v`.
3. The `release` workflow builds every platform, runs `scripts/check-deb.sh` and
   `scripts/build-freebsd-pkg.sh`, and attaches the tarballs, `.deb`s and `.pkg`;
   `workflow_dispatch` with `dry_run: "true"` skips the release itself.
4. Its last job regenerates the Homebrew formula from the published tarballs and pushes it to
   [seitzbg/homebrew-mtr-rs](https://github.com/seitzbg/homebrew-mtr-rs). That needs the
   `HOMEBREW_TAP_TOKEN` repository secret: a fine-grained personal access token for the tap
   repository with Contents read and write. Without it the job warns and skips, and
   `scripts/homebrew-formula.sh <version>` produces the same file by hand.

## Credits

mtr-rs was written by Bryan Seitz with assistance from Claude (Anthropic). It is a port of
[mtr](https://github.com/traviscross/mtr), created by Matt Kimball, maintained for many years by
Roger Wolff and now by Travis Cross and the mtr contributors; the protocol, the probe engine and the
report formats are theirs. Both projects are GPL-2.0-only.
