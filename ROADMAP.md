# Roadmap

Planned work, roughly in order. Items move to `CHANGELOG.md` when they ship; nothing here is a
promise of a date.

## 0.3

- Themes: a `[theme]` section in the config file, and `--theme NAME`, mapping the semantic styles
  (ok / warn / bad / alert / accent / dim / selected) to named ANSI colours, 256-colour indexes or
  RGB, with a few built-in presets and the terminal's own ANSI palette as the default. Today every
  style is a fixed named ANSI colour, so the terminal theme decides.

## Later

- IPv6 compat cases in CI need a self-hosted or IPv6-capable runner.

## 2.0

- A pty-driven end-to-end test of the default (TUI) invocation.
- Windows client (report modes only) driving a remote or WSL helper.
