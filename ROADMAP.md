# Roadmap

Planned work, roughly in order. Items move to `CHANGELOG.md` when they ship; nothing here is a
promise of a date.

## Where things stand (2026-09-05)

0.3.0 is released: Linux (x86_64, aarch64), macOS (Apple silicon, Intel) and FreeBSD (x86_64), with
`.deb` and `.pkg` packages and a Homebrew tap ([seitzbg/homebrew-mtr-rs](https://github.com/seitzbg/homebrew-mtr-rs)).
CI runs the full suite as root on all three OSes, so the raw-socket loopback probe tests execute
everywhere. The release workflow regenerates and pushes the Homebrew formula itself; the
`HOMEBREW_TAP_TOKEN` secret is set, and the next `v*` tag is the first time that job runs for real.
Known gaps carried forward: macOS binaries are unsigned (users clear the Gatekeeper quarantine flag
by hand), releases carry no checksums or provenance, and FreeBSD aarch64 has no binary (it builds
from source; GitHub cannot virtualise it and emulation took over an hour).

## 0.4

- Sign and notarize the macOS binaries in `build-macos` (Developer ID certificate and an
  app-specific password as secrets; `codesign`, then `notarytool`), so the `xattr` step in the
  README goes away.
- Release integrity: a `SHA256SUMS` asset and build provenance attestations
  (`actions/attest-build-provenance`); `scripts/homebrew-formula.sh` can then read the checksums
  instead of hashing the tarballs itself.
- Themes: a `[theme]` section in the config file, and `--theme NAME`, mapping the semantic styles
  (ok / warn / bad / alert / accent / dim / selected) to named ANSI colours, 256-colour indexes or
  RGB, with a few built-in presets and the terminal's own ANSI palette as the default. Today every
  style is a fixed named ANSI colour, so the terminal theme decides.

## Later

- FreeBSD aarch64 release binaries by cross-compiling from Linux (`aarch64-unknown-freebsd` is a
  Tier 3 target: nightly `-Zbuild-std` plus a sysroot from `base.txz`), if anyone asks.
- IPv6 compat cases in CI need a self-hosted or IPv6-capable runner.
- An unprivileged macOS path: Darwin has `SOCK_DGRAM`/`IPPROTO_ICMP` sockets like Linux but no
  `IP_RECVERR`, so it would hear echo replies only; worth measuring before deciding.

## 2.0

- A pty-driven end-to-end test of the default (TUI) invocation.
- Windows client (report modes only) driving a remote or WSL helper.
