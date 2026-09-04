#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-only
# Install (or remove) mtr-rs: both binaries, man pages, completions, and the capability
# mtr-packet needs for raw sockets.
#
#   scripts/install.sh [--prefix DIR] [--no-build] [--no-setcap] [--uninstall]
#
# Defaults: --prefix /usr/local. Root is needed only for a system prefix and for setcap.
set -euo pipefail

prefix=/usr/local
build=1
setcap=1
uninstall=0
while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) prefix=${2:?--prefix needs a directory}; shift 2 ;;
    --prefix=*) prefix=${1#--prefix=}; shift ;;
    --no-build) build=0; shift ;;
    --no-setcap) setcap=0; shift ;;
    --uninstall) uninstall=1; shift ;;
    -h|--help)
      cat <<'EOF'
Install (or remove) mtr-rs: both binaries, man pages, completions, and the capability
mtr-packet needs for raw sockets.

  scripts/install.sh [--prefix DIR] [--no-build] [--no-setcap] [--uninstall]

Defaults: --prefix /usr/local. Root is needed only for a system prefix and for setcap.
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

if [ "$build" = 1 ]; then
  (cd "$root" && cargo build --release --workspace)
fi
for bin in mtr mtr-packet; do
  [ -x "$root/target/release/$bin" ] || { echo "install.sh: missing $root/target/release/$bin (run without --no-build)" >&2; exit 1; }
done
(cd "$root" && cargo xtask man >/dev/null && cargo xtask completions >/dev/null)

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
  if command -v setcap >/dev/null 2>&1 && setcap cap_net_raw+ep "$bindir/mtr-packet" 2>/dev/null; then
    echo "granted cap_net_raw to $bindir/mtr-packet"
  else
    cat >&2 <<EOF
install.sh: could not grant cap_net_raw (missing setcap or not root). mtr-packet still works on
its unprivileged fallback; for raw sockets (MPLS labels, TCP/SCTP hop discovery) run:
    sudo setcap cap_net_raw+ep $bindir/mtr-packet
EOF
  fi
fi
