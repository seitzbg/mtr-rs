#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-only
# Install (or remove) mtr-rs: both binaries, man pages, completions, and the capability
# mtr-packet needs for raw sockets.
#
#   scripts/install.sh [--prefix DIR] [--no-build] [--no-setcap] [--uninstall]
#
# Defaults: --prefix /usr/local. Root is needed only for a system prefix and for setcap, so
# a system install is two phases: build the artefacts as yourself, then copy them as root:
#
#   cargo build --release --workspace && cargo xtask dist
#   sudo scripts/install.sh --no-build
#
# With --no-build nothing is compiled and cargo is never run, so it is safe under sudo.
set -euo pipefail

prefix=/usr/local
build=1
setcap=1
uninstall=0
while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) prefix=${2:?--prefix needs a directory}; shift 2 ;;
    --prefix=*) prefix=${1#--prefix=}; [ -n "$prefix" ] || { echo "install.sh: --prefix= needs a directory" >&2; exit 1; }; shift ;;
    --no-build) build=0; shift ;;
    --no-setcap) setcap=0; shift ;;
    --uninstall) uninstall=1; shift ;;
    -h|--help)
      cat <<'EOF'
Install (or remove) mtr-rs: both binaries, man pages, completions, and the capability
mtr-packet needs for raw sockets.

  scripts/install.sh [--prefix DIR] [--no-build] [--no-setcap] [--uninstall]

Defaults: --prefix /usr/local. Root is needed only for a system prefix and for setcap, so
a system install is two phases: build the artefacts as yourself, then copy them as root:

  cargo build --release --workspace && cargo xtask dist
  sudo scripts/install.sh --no-build

With --no-build nothing is compiled and cargo is never run, so it is safe under sudo.
EOF
      exit 0 ;;
    *) echo "install.sh: unknown option $1" >&2; exit 1 ;;
  esac
done

root=$(cd "$(dirname "$0")/.." && pwd)
bindir=$prefix/bin
mandir=$prefix/share/man/man8
bashdir=$prefix/share/bash-completion/completions
zshdir=$prefix/share/zsh/site-functions
fishdir=$prefix/share/fish/vendor_completions.d

# Every path this script owns; --uninstall removes exactly these.
files=(
  "$bindir/mtr" "$bindir/mtr-packet"
  "$mandir/mtr.8" "$mandir/mtr-packet.8"
  "$bashdir/mtr" "$zshdir/_mtr" "$fishdir/mtr.fish"
)

if [ "$uninstall" = 1 ]; then
  for f in "${files[@]}"; do
    if [ -e "$f" ]; then rm -f "$f"; echo "removed $f"; fi
  done
  exit 0
fi

# The generated artefacts install.sh copies: present already under --no-build, regenerated
# otherwise. Under --no-build we must not invoke cargo at all -- that is the whole point of
# the flag (the caller is typically root, and building as root would poison ./target).
assets=(
  "$root/target/dist/man/mtr.8" "$root/target/dist/man/mtr-packet.8"
  "$root/target/dist/completions/mtr.bash" "$root/target/dist/completions/_mtr"
  "$root/target/dist/completions/mtr.fish"
)

if [ "$build" = 1 ]; then
  (cd "$root" && cargo build --release --workspace)
fi
for bin in mtr mtr-packet; do
  [ -x "$root/target/release/$bin" ] || { echo "install.sh: missing $root/target/release/$bin (run without --no-build, or build first: cargo build --release --workspace)" >&2; exit 1; }
done
if [ "$build" = 1 ]; then
  (cd "$root" && cargo xtask man >/dev/null && cargo xtask completions >/dev/null)
else
  for f in "${assets[@]}"; do
    [ -f "$f" ] || { echo "install.sh: missing $f -- generate it first (as yourself, not root): cargo xtask dist" >&2; exit 1; }
  done
fi

install -d "$bindir" "$mandir" "$bashdir" "$zshdir" "$fishdir"
install -m 755 "$root/target/release/mtr" "$bindir/mtr"
install -m 755 "$root/target/release/mtr-packet" "$bindir/mtr-packet"
install -m 644 "$root/target/dist/man/mtr.8" "$mandir/mtr.8"
install -m 644 "$root/target/dist/man/mtr-packet.8" "$mandir/mtr-packet.8"
install -m 644 "$root/target/dist/completions/mtr.bash" "$bashdir/mtr"
install -m 644 "$root/target/dist/completions/_mtr" "$zshdir/_mtr"
install -m 644 "$root/target/dist/completions/mtr.fish" "$fishdir/mtr.fish"
for f in "${files[@]}"; do echo "installed $f"; done

if [ "$setcap" = 1 ]; then
  if command -v setcap >/dev/null 2>&1 && setcap cap_net_raw+ep "$bindir/mtr-packet"; then
    echo "granted cap_net_raw to $bindir/mtr-packet"
  else
    cat >&2 <<EOF
install.sh: could not grant cap_net_raw (see any setcap error above; usually a missing setcap
or not root). mtr-packet still works on
its unprivileged fallback; for raw sockets (MPLS labels, TCP/SCTP hop discovery) run:
    sudo setcap cap_net_raw+ep $bindir/mtr-packet
EOF
  fi
fi
