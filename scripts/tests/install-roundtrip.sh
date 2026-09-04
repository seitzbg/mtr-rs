#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-only
# Install into a temp prefix with --no-build/--no-setcap, check every file, uninstall, check gone.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
prefix=$(mktemp -d)
trap 'rm -rf "$prefix"' EXIT

"$root/scripts/install.sh" --prefix "$prefix" --no-build --no-setcap

expected=(
  bin/mtr-rs bin/mtr-rs-packet
  share/man/man8/mtr-rs.8 share/man/man8/mtr-rs-packet.8
  share/bash-completion/completions/mtr-rs
  share/zsh/site-functions/_mtr-rs
  share/fish/vendor_completions.d/mtr-rs.fish
)
for f in "${expected[@]}"; do
  [ -e "$prefix/$f" ] || { echo "missing $prefix/$f" >&2; exit 1; }
done
[ -x "$prefix/bin/mtr-rs" ] && [ -x "$prefix/bin/mtr-rs-packet" ] || { echo "binaries not executable" >&2; exit 1; }
"$prefix/bin/mtr-rs" --version | grep -q '^mtr-rs ' || { echo "installed mtr-rs does not run" >&2; exit 1; }

"$root/scripts/install.sh" --prefix "$prefix" --uninstall
for f in "${expected[@]}"; do
  [ ! -e "$prefix/$f" ] || { echo "still present after uninstall: $prefix/$f" >&2; exit 1; }
done
echo "install round-trip ok"
