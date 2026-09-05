# Changelog

All notable changes to mtr-rs are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- The release workflow regenerates the Homebrew formula from the published tarballs and pushes it
  to [seitzbg/homebrew-mtr-rs](https://github.com/seitzbg/homebrew-mtr-rs); without the
  `HOMEBREW_TAP_TOKEN` secret it warns and skips. `scripts/homebrew-formula.sh` does the same by
  hand.
- README: per-platform install steps (Homebrew first for macOS) and a "Checking the install"
  walkthrough covering every probe mode with its expected output.
- Release integrity: every release now carries a `SHA256SUMS` asset and a GitHub build-provenance
  attestation for each file (`gh attestation verify <file> --repo seitzbg/mtr-rs`).
  `scripts/homebrew-formula.sh` reads the checksums from `SHA256SUMS` instead of hashing the
  tarballs itself, falling back to hashing for releases that predate the file.

### Changed
- The declared minimum Rust version is 1.88. The `Cargo.toml` said 1.85, but the config-file
  merge uses let-chains (stable since 1.88) and ratatui 0.30 and hickory 0.26 require 1.88 too,
  so a 1.85 build never worked. CI now checks the workspace on 1.88 so the floor stays true.
- FreeBSD release binaries are x86_64 only; aarch64 builds and tests from source but the emulated
  release build took over an hour per tag.
- The FreeBSD CI job no longer copies the workspace back out of the VM (a spurious `rsync`
  failure after every test had passed).

### Fixed
- The TUI interval prompt stored milliseconds as `u32`, so any value above about 49.7 days was
  silently clamped even though it was accepted up to the one-year ceiling. The action now carries
  `u64` milliseconds.

## [0.3.0] - 2026-09-05

### Added
- FreeBSD support: the helper opens the same raw sockets as on Linux and drops to the invoking
  user with `setuid()`; `scripts/install.sh` makes it setuid root, and the release ships an
  x86_64 tarball and pkg(8) package (`packaging/freebsd/`). aarch64 builds from source (tested
  on FreeBSD 14.3 arm64) but has no release binary, since GitHub cannot virtualise it. CI runs
  the whole suite as root in a FreeBSD VM, so the loopback probe tests execute there.
- macOS support (Apple silicon and Intel): the stub backend is gone; the helper is the same
  raw-socket Unix backend as on FreeBSD, setuid root, with `local-device` honoured through
  `IP_BOUND_IF`. CI runs the suite as root on GitHub's macOS runners and the release ships
  macOS tarballs.

### Changed
- Release tarballs are named `mtr-rs-<version>-<arch>-<os>.tar.gz` (`...-x86_64-linux`,
  `...-x86_64-freebsd`, `...-aarch64-macos`); the arch alone no longer identifies a build.
- `-M`/`--mark` is refused outside Linux with a message saying so, instead of failing later in
  the helper; `SO_MARK` has no equivalent there. The helper answers a `mark` or `local-device`
  it cannot honour with `invalid-argument` rather than silently probing without it.
- The helper's privilege hints name the fix for the running OS: `setcap` and `ping_group_range` on
  Linux, `chmod u+s` on FreeBSD and macOS.

### Fixed
- `--init-config` rejects `max_unknown` and `timeout` values above the runtime's integer range
  instead of writing a configuration that the next invocation cannot use.
- Pull requests that change the shared Unix ICMP parser trigger the parser fuzz smoke workflow
  after the backend's `linux` to `unix` rename.

### Security
- Under the sudo marker, helper discovery no longer searches `PATH` or the current directory; it
  uses only absolute paths beside the running client or in the standard installation directories.

## [0.2.1] - 2026-09-04

### Changed
- The TUI uses the terminal's named ANSI colours at every colour depth, instead of its own muted RGB
  set on truecolor terminals, so the terminal theme decides and an ssh session looks like a local one.
- A lost sample in the Recent sparkline is a red floor `▁` flush with the bars (`_` under `--ascii`),
  so a lossy hop reads as a continuous red line. Without colour the `•`/`x` mark stays, because the
  floor would match the lowest RTT bucket. Hops that never answered stay blank.

## [0.2.0] - 2026-09-04

### Added
- The client names both fixes when the helper cannot open its sockets: `setcap cap_net_raw`, or a
  wider `net.ipv4.ping_group_range`.

### Changed
- The TUI header reserves room for the `[PAUSED]` marker next to the clock, so it stays visible at
  80 columns; the left part of the header is truncated instead.
- CI gates on the upstream `probe.py` suite (IPv4) instead of only reporting it.
- The installed binaries are `mtr-rs` and `mtr-rs-packet`, and the man pages, completions and setcap
  hints follow. The Debian package installs alongside the distribution's mtr and no longer declares
  Conflicts/Provides/Replaces on `mtr`/`mtr-tiny`. The client looks for `mtr-rs-packet` first and
  still falls back to a C `mtr-packet` on the path.
- `--version`, the TUI header and the error messages say `mtr-rs`; CSV and JSON output keep the C
  names.
- `--init-config` writes the options in effect (defaults, `MTR_OPTIONS`, command line) rather than
  the defaults alone. A key you changed is written uncommented and the rest stay commented out at
  their default. Options that would not load or run again (`-o LSQ`, or `-i 0.5` as a non-root user)
  are refused instead of written.
- `scripts/install.sh` removes a pre-0.2 install of this project's `mtr`/`mtr-packet` files on
  upgrade or uninstall, after checking that the binary is ours.

### Fixed
- Each target name is resolved once, and the same-family check reuses the addresses it looked up.
- `--init-config` saves the clamped `max_ttl` that is actually in effect.

### Security
- CI actions moved to their current majors; fuzz targets run for 20 s each on pull requests that
  touch the parsers.

## [0.1.0] - 2026-09-04

First release of the Rust port of mtr 0.96 (upstream commit 7b01773).

### Added
- `mtr` client: interactive ratatui TUI (the default), classic `-r` report, `-w` wide report, `-j`
  JSON and `-C` CSV modes with the same output as the C client; `--report-on-exit`; AS-number and
  AS-name lookup; absolute RTT colour thresholds (`--rtt-thresholds`, default 30/100/200/500 ms);
  `--ascii`, `--color auto|always|never`, `NO_COLOR`.
- Config file `~/.config/mtr-rs/config.toml` (`--init-config`, `--config PATH`) with a `[display]`
  and a `[probe]` section (`interval`, `gracetime`, `max_ttl`, `max_unknown`, `timeout`, `dns`,
  `asn`); precedence defaults < file < `MTR_OPTIONS` < command line.
- `mtr-packet` helper in Rust, wire-compatible with the C helper of mtr 0.96: ICMP/UDP/TCP/SCTP
  probes over IPv4/IPv6, MPLS extension decoding (RFC 4884/4950), a full privilege drop once the
  sockets are open, and an unprivileged `SOCK_DGRAM` + `IP_RECVERR` fallback on Linux.
- Packaging: `cargo xtask man|completions|dist`, `scripts/install.sh`, Debian package `mtr-rs`, and
  a release workflow producing x86_64 and aarch64 tarballs and `.deb`s on `v*` tags.
- Fuzz targets for the line protocol (`crates/mtr-proto/fuzz`) and the ICMP/MPLS parsers
  (`crates/mtr-packet/fuzz`), run nightly.
- Linux only: the `mtr-packet` helper needs Linux sockets, and the macOS backend is a stub (see
  `ROADMAP.md`).

### Changed
- mtr-packet reports `mark` support only when it holds CAP_NET_ADMIN, and keeps that one capability
  across the privilege drop when granted, so `--mark` works.
- `-j` with several targets prints one JSON array instead of concatenated objects.

### Fixed
- `-i`, `-G`, the config file and the TUI interval prompt reject `nan`, `inf` and absurdly large
  values instead of freezing the schedule.
- Several targets in interactive mode: only the first runs and a resolution failure is fatal, as in
  C. The same-family check applies to the report modes only, where any per-target failure skips to
  the next target as in C.
- CSV output quotes host names, PTR names and AS text that contain commas, quotes or newlines
  (RFC 4180).
- Out-of-range `-M`, `-c`, `-U`, `-Z` and `--rtt-thresholds` values are rejected with a range error
  instead of silently wrapping or clamping. A negative value written as a separate word (`-M -1`,
  which C's getopt accepts) reaches that check instead of being read as an unknown flag, and, as in
  C's getopt, a numeric option consumes a following `-4`/`-6` as its value.
- AS lookups treat only `64:ff9b::/96` as NAT64; C checks 32 bits.
- mtr-packet parses IPv4 packets and quoted headers with IP options (IHL > 5) instead of assuming
  20 bytes.

### Security
- `MTR_RS_LOG` is ignored when `/etc/mtr.is.run.under.sudo` exists, and never truncates or follows
  an existing file.
- GitHub Actions pinned to commit SHAs, read-only tokens except for the release publish step, and
  cargo-deny in CI.

[Unreleased]: https://github.com/seitzbg/mtr-rs/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/seitzbg/mtr-rs/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/seitzbg/mtr-rs/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/seitzbg/mtr-rs/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/seitzbg/mtr-rs/releases/tag/v0.1.0
