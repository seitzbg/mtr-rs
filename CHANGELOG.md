# Changelog

All notable changes to mtr-rs are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed
- Installed binaries are now `mtr-rs` and `mtr-rs-packet` (man pages, completions and setcap hints follow); the Debian package installs alongside the distribution's mtr and no longer declares Conflicts/Provides/Replaces on `mtr`/`mtr-tiny`. The client looks for `mtr-rs-packet` first and still falls back to a C `mtr-packet` on the path.
- `--version`, the TUI header and error messages say `mtr-rs`; CSV and JSON output keep the C names.

## [0.1.0] - 2026-09-04

First release of the Rust port of mtr 0.96 (upstream commit 7b01773).

### Added
- `mtr` client: interactive ratatui TUI (default), classic `-r` report, `-w` wide report, `-j` JSON
  and `-C` CSV modes with the same output as the C client; `--report-on-exit`; AS-number and
  AS-name lookup; absolute RTT colour thresholds (`--rtt-thresholds`, default 30/100/200/500 ms);
  `--ascii`, `--color auto|always|never`, `NO_COLOR`.
- Config file `~/.config/mtr-rs/config.toml` (`--init-config`, `--config PATH`) with a
  `[display]` and a `[probe]` section (`interval`, `gracetime`, `max_ttl`, `max_unknown`,
  `timeout`, `dns`, `asn`); precedence defaults < file < `MTR_OPTIONS` < command line.
- `mtr-packet` helper in Rust, wire-compatible with the C helper of mtr 0.96: ICMP/UDP/TCP/SCTP
  probes over IPv4/IPv6, MPLS extension decoding (RFC 4884/4950), full privilege drop after the
  sockets are open, unprivileged `SOCK_DGRAM` + `IP_RECVERR` fallback on Linux.
- Packaging: `cargo xtask man|completions|dist`, `scripts/install.sh`, Debian package `mtr-rs`,
  release workflow producing x86_64 and aarch64 tarballs and `.deb`s on `v*` tags.
- Fuzz targets for the line protocol (`crates/mtr-proto/fuzz`) and the ICMP/MPLS parsers
  (`crates/mtr-packet/fuzz`), run nightly.
- Linux only: the `mtr-packet` helper needs Linux sockets; the macOS backend is a stub (see `ROADMAP.md`).

### Changed
- mtr-packet reports `mark` support only when it actually holds CAP_NET_ADMIN, and keeps that one capability across the privilege drop when granted, so `--mark` works (deviation 34, CR-03).
- `-j` with several targets prints one JSON array instead of concatenated objects (deviation 30, CR-04).

### Fixed
- `-i`, `-G`, the config file and the TUI interval prompt reject `nan`, `inf` and absurdly large values instead of freezing the schedule (CR-02).
- Several targets in interactive mode: only the first runs and a resolution failure is fatal, as in C; the same-family check applies to the report modes only (CR-05), and any per-target failure in the report modes skips to the next target as in C.
- CSV output quotes host names, PTR names and AS text that contain commas, quotes or newlines (RFC 4180; deviation 31, CR-06).
- Out-of-range `-M`, `-c`, `-U`, `-Z`, `--rtt-thresholds` values are rejected with a range error instead of silently wrapping or clamping (deviation 35 for `-U`/`-c`), and a negative value written as a separate word (`-M -1`, which C's getopt accepts) reaches that check instead of being read as an unknown flag; as in C's getopt, a numeric option also consumes a following `-4`/`-6` as its value (CR-10).
- AS lookups treat only `64:ff9b::/96` as NAT64 (C checks 32 bits; deviation 33, CR-09).
- mtr-packet parses IPv4 packets and quoted headers with IP options (IHL > 5) instead of assuming 20 bytes (deviation 32, CR-08).

### Security
- `MTR_RS_LOG` is ignored when `/etc/mtr.is.run.under.sudo` exists and never truncates or follows an existing file (CR-01).
- GitHub Actions pinned to commit SHAs, read-only tokens except the release publish step, and cargo-deny in CI (CR-07).

[Unreleased]: https://github.com/seitzbg/mtr-rs/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/seitzbg/mtr-rs/releases/tag/v0.1.0
