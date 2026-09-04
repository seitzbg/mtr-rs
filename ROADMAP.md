# Roadmap

Planned work, roughly in order. Items move to `CHANGELOG.md` when they ship; nothing here is a
promise of a date.

## Later

- IPv6 compat cases in CI need a self-hosted or IPv6-capable runner.
- (Shipped in 0.1.0: footer key hints keep the `n`/`z`/`e` toggles visible at 80 columns.)

## 2.0

- macOS backend for `mtr-rs-packet` (the `cfg(target_os = "macos")` stub exists; BPF/raw-socket
  implementation does not).
- A pty-driven end-to-end test of the default (TUI) invocation.
- Windows client (report modes only) driving a remote or WSL helper.
