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

# A pre-0.2 install of this project is cleared out by the next install.
mkdir -p "$prefix/bin" "$prefix/share/man/man8"
printf '#!/bin/sh\necho "mtr 0.1.0"\n' > "$prefix/bin/mtr"
chmod 755 "$prefix/bin/mtr"
: > "$prefix/bin/mtr-packet"
: > "$prefix/share/man/man8/mtr.8"
"$root/scripts/install.sh" --prefix "$prefix" --no-build --no-setcap
for f in bin/mtr bin/mtr-packet share/man/man8/mtr.8; do
  [ ! -e "$prefix/$f" ] || { echo "legacy file not removed: $prefix/$f" >&2; exit 1; }
done

# Someone else's mtr on the same prefix (the distribution's C client) is left alone.
printf '#!/bin/sh\necho "mtr 0.95"\n' > "$prefix/bin/mtr"
chmod 755 "$prefix/bin/mtr"
: > "$prefix/bin/mtr-packet"
out=$("$root/scripts/install.sh" --prefix "$prefix" --no-build --no-setcap)
grep -q "left $prefix/bin/mtr alone" <<<"$out" \
  || { echo "install.sh did not report leaving a foreign mtr alone" >&2; exit 1; }
[ -e "$prefix/bin/mtr" ] && [ -e "$prefix/bin/mtr-packet" ] \
  || { echo "install.sh removed an mtr that is not ours" >&2; exit 1; }
"$root/scripts/install.sh" --prefix "$prefix" --uninstall >/dev/null
[ -e "$prefix/bin/mtr" ] || { echo "--uninstall removed an mtr that is not ours" >&2; exit 1; }

echo "install round-trip ok"
