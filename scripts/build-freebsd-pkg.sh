#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-only
# Build the FreeBSD package (reusing target/release and target/dist) and assert its contents.
# The FreeBSD counterpart of check-deb.sh: `pkg create` from packaging/freebsd/, then check the
# file list, the setuid bit on the helper and the metadata. Prints the package path last.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
if [ "$(uname -s)" != FreeBSD ] || ! command -v pkg >/dev/null 2>&1; then
  echo "skipping: needs FreeBSD with pkg(8)"
  exit 0
fi
for bin in mtr-rs mtr-rs-packet; do
  [ -x "target/release/$bin" ] || { echo "missing target/release/$bin (cargo build --release --workspace)" >&2; exit 1; }
done
cargo xtask dist --no-build >/dev/null

version=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
# Wildcard the OS-version field of the ABI (e.g. FreeBSD:14:amd64 -> FreeBSD:*:amd64) so one
# package installs across FreeBSD major versions; pkg on 15-STABLE/15.1 otherwise rejects a
# package built against a different major.
abi=$(pkg config ABI | sed -E 's/^([^:]+):[^:]+:/\1:*:/')
stage=target/freebsd/stage
out=target/freebsd
rm -rf "$stage"
p="$stage/usr/local"
install -d "$p/bin" "$p/share/man/man8" "$p/share/bash-completion/completions" \
  "$p/share/zsh/site-functions" "$p/share/fish/vendor_completions.d" "$p/share/doc/mtr-rs"
install -m 755 target/release/mtr-rs "$p/bin/mtr-rs"
# The mode in the staging tree does not matter: pkg-plist's @(root,wheel,4755) is what the
# package records, and `pkg install` applies it as root.
install -m 755 target/release/mtr-rs-packet "$p/bin/mtr-rs-packet"
for m in mtr-rs mtr-rs-packet; do gzip -9 -n -c "target/dist/man/$m.8" > "$p/share/man/man8/$m.8.gz"; done
install -m 644 target/dist/completions/mtr-rs.bash "$p/share/bash-completion/completions/mtr-rs"
install -m 644 target/dist/completions/_mtr-rs "$p/share/zsh/site-functions/_mtr-rs"
install -m 644 target/dist/completions/mtr-rs.fish "$p/share/fish/vendor_completions.d/mtr-rs.fish"
install -m 644 docs/config.example.toml "$p/share/doc/mtr-rs/config.example.toml"

sed -e "s/@VERSION@/$version/" -e "s/@ABI@/$abi/" packaging/freebsd/MANIFEST.in > "$out/+MANIFEST"
pkg create -M "$out/+MANIFEST" -p packaging/freebsd/pkg-plist -r "$stage" -o "$out" >/dev/null
pkgfile="$out/mtr-rs-$version.pkg"
[ -f "$pkgfile" ] || { echo "pkg create produced no $pkgfile" >&2; ls -l "$out" >&2; exit 1; }

# Every file in pkg-plist is in the package, at the prefix.
files=$(pkg info -l -F "$pkgfile")
sed 's/^@([^)]*) //' packaging/freebsd/pkg-plist | while read -r f; do
  echo "$files" | grep -q "/usr/local/$f\$" || { echo "package lacks /usr/local/$f" >&2; exit 1; }
done
# The helper is setuid root: without that only root can run mtr-rs on FreeBSD.
tar tvf "$pkgfile" | grep -E '^-rwsr-xr-x +[0-9]+ +root +wheel .*/usr/local/bin/mtr-rs-packet$' >/dev/null \
  || { echo "mtr-rs-packet is not setuid root in the package" >&2; tar tvf "$pkgfile" | grep mtr-rs-packet >&2; exit 1; }
tar tvf "$pkgfile" | grep -E ' /usr/local/bin/mtr-rs$' | grep -qv 'rws' || { echo "mtr-rs must not be setuid" >&2; exit 1; }
info=$(pkg info -F "$pkgfile")
echo "$info" | grep -q "^Version *: $version\$" || { echo "wrong version in package" >&2; echo "$info" >&2; exit 1; }
echo "$info" | grep -q '^Licenses *: GPLv2' || { echo "licence missing" >&2; echo "$info" >&2; exit 1; }
# Compare the Architecture field literally: a wildcarded ABI contains '*', which grep would treat
# as a regex quantifier.
arch=$(echo "$info" | sed -n 's/^Architecture *: *//p')
[ "$arch" = "$abi" ] || { echo "wrong ABI in package (got '$arch', want '$abi')" >&2; echo "$info" >&2; exit 1; }
echo "pkg ok"
echo "$pkgfile"
