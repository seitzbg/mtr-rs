# Roadmap

Planned work, roughly in order. Items move to `CHANGELOG.md` when they ship; nothing here is a
promise of a date.

## 0.2

- Footer key hints: keep the `n`/`z`/`e` toggles visible at 80 columns.
- Remote IPv6 cases of the upstream compat suites in CI (need an IPv6-capable runner).

## 2.0

- macOS backend for `mtr-rs-packet` (the `cfg(target_os = "macos")` stub exists; BPF/raw-socket
  implementation does not).
- A pty-driven end-to-end test of the default (TUI) invocation.
- Windows client (report modes only) driving a remote or WSL helper.
