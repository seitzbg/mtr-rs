#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-only
# Install into a temp prefix with --no-build/--no-setcap, check every file, uninstall, check gone.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
prefix=$(mktemp -d)
trap 'rm -rf "$prefix"' EXIT

"$root/scripts/install.sh" --prefix "$prefix" --no-build --no-setcap

expected=(
  bin/mtr bin/mtr-packet
  share/man/man8/mtr.8 share/man/man8/mtr-packet.8
  share/bash-completion/completions/mtr
  share/zsh/site-functions/_mtr
  share/fish/vendor_completions.d/mtr.fish
)
for f in "${expected[@]}"; do
  [ -e "$prefix/$f" ] || { echo "missing $prefix/$f" >&2; exit 1; }
done
[ -x "$prefix/bin/mtr" ] && [ -x "$prefix/bin/mtr-packet" ] || { echo "binaries not executable" >&2; exit 1; }
"$prefix/bin/mtr" --version | grep -q '^mtr ' || { echo "installed mtr does not run" >&2; exit 1; }

"$root/scripts/install.sh" --prefix "$prefix" --uninstall
for f in "${expected[@]}"; do
  [ ! -e "$prefix/$f" ] || { echo "still present after uninstall: $prefix/$f" >&2; exit 1; }
done
echo "install round-trip ok"
