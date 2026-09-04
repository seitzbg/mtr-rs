# Changelog

All notable changes to mtr-rs are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
### Changed
- `-j` with several targets prints one JSON array instead of concatenated objects (deviation 30, CR-04).
### Fixed
- `-i`, `-G`, the config file and the TUI interval prompt reject `nan`, `inf` and absurdly large values instead of freezing the schedule (CR-02).
- Several targets in interactive mode: only the first runs and a resolution failure is fatal, as in C; the same-family check applies to the report modes only (CR-05).
- CSV output quotes host names, PTR names and AS text that contain commas, quotes or newlines (RFC 4180; deviation 31, CR-06).
- Out-of-range `-M`, `-c`, `-U`, `-Z`, `--rtt-thresholds` values are rejected instead of silently wrapping (CR-10).
- AS lookups treat only `64:ff9b::/96` as NAT64 (C checks 32 bits; deviation 33, CR-09).
### Security
- `MTR_RS_LOG` is ignored when `/etc/mtr.is.run.under.sudo` exists and never truncates or follows an existing file (CR-01).
- GitHub Actions pinned to commit SHAs, read-only tokens except the release publish step, and cargo-deny in CI (CR-07).

## [0.1.0] - Unreleased

First release of the Rust port of mtr 0.96 (upstream commit 7b01773).

### Added
- `mtr` client: interactive ratatui TUI (default), classic `-r` report, `-w` wide report, `-j` JSON
  and `-C` CSV modes with the same output as the C client; `--report-on-exit`; AS-number and
  AS-name lookup; absolute RTT colour thresholds (`--rtt-thresholds`, default 30/100/200/500 ms);
  `--ascii`, `--color auto|always|never`, `NO_COLOR`.
- Config file `~/.config/mtr-rs/config.toml` (`--init-config`, `--config PATH`); precedence
  defaults < file < `MTR_OPTIONS` < command line.
- `mtr-packet` helper in Rust, wire-compatible with the C helper of mtr 0.96: ICMP/UDP/TCP/SCTP
  probes over IPv4/IPv6, MPLS extension decoding (RFC 4884/4950), full privilege drop after the
  sockets are open, unprivileged `SOCK_DGRAM` + `IP_RECVERR` fallback on Linux.
- Packaging: `cargo xtask man|completions|dist`, `scripts/install.sh`, Debian package `mtr-rs`,
  release workflow producing x86_64 and aarch64 tarballs and `.deb`s on `v*` tags.
- Fuzz targets for the line protocol (`crates/mtr-proto/fuzz`) and the ICMP/MPLS parsers
  (`crates/mtr-packet/fuzz`), run nightly.

[Unreleased]: https://github.com/seitzbg/mtr-rs/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/seitzbg/mtr-rs/releases/tag/v0.1.0
