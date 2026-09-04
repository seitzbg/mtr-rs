# Roadmap

Planned work, roughly in order. Items move to `CHANGELOG.md` when they ship; nothing here is a
promise of a date.

## Next (0.1.x)

- Fixes from the September 2026 external code review (tracked as CR-01..CR-10 in the pull request
  that adds this file): logging under the sudo guard, non-finite interval/gracetime, multi-target
  behaviour in interactive mode, valid multi-target JSON, CSV quoting, IPv4 options (IHL), NAT64
  `/96`, `--mark` support reporting, numeric range checks, pinned CI actions and `cargo-deny`.

## 0.2

- Resolve each target once: the same-family check and the run both call `getaddrinfo` today.
- `--init-config` writes the current effective options, not only the defaults.
- Footer key hints: keep the `n`/`z`/`e` toggles visible at 80 columns.
- Remote IPv6 cases of the upstream compat suites in CI (need an IPv6-capable runner).

## 2.0

- macOS backend for `mtr-packet` (the `cfg(target_os = "macos")` stub exists; BPF/raw-socket
  implementation does not).
- A pty-driven end-to-end test of the default (TUI) invocation.
- Windows client (report modes only) driving a remote or WSL helper.
