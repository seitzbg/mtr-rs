#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-only
# Build the .deb (reusing target/release) and assert its contents and control fields.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
if ! command -v cargo-deb >/dev/null 2>&1; then
  echo "skipping: cargo-deb not installed (cargo install cargo-deb --locked)"
  exit 0
fi
cargo xtask dist --no-build >/dev/null
deb=$(cargo deb -p mtr --no-build | tail -1)
[ -f "$deb" ] || { echo "cargo deb produced no file" >&2; exit 1; }
echo "built $deb"

contents=$(dpkg-deb -c "$deb")
for path in ./usr/bin/mtr ./usr/bin/mtr-packet ./usr/share/man/man8/mtr.8.gz \
            ./usr/share/man/man8/mtr-packet.8.gz ./usr/share/bash-completion/completions/mtr \
            ./usr/share/zsh/site-functions/_mtr ./usr/share/fish/vendor_completions.d/mtr.fish \
            ./usr/share/doc/mtr-rs/config.example.toml; do
  grep -q " $path$" <<<"$contents" || { echo "missing from package: $path" >&2; exit 1; }
done
grep -qE '^-rwxr-xr-x .* \./usr/bin/mtr-packet$' <<<"$contents" || { echo "mtr-packet is not 755" >&2; exit 1; }

info=$(dpkg-deb -I "$deb")
grep -q '^ Package: mtr-rs$' <<<"$info" || { echo "package name is not mtr-rs" >&2; exit 1; }
grep -q '^ Conflicts: mtr, mtr-tiny$' <<<"$info" || { echo "Conflicts missing" >&2; exit 1; }
grep -q '^ Section: net$' <<<"$info" || { echo "Section is not net" >&2; exit 1; }
grep -qE '^ Depends: .*libc6' <<<"$info" || { echo "Depends not auto-computed" >&2; exit 1; }

ctl=$(mktemp -d); trap 'rm -rf "$ctl"' EXIT
dpkg-deb -e "$deb" "$ctl"
grep -q 'setcap cap_net_raw+ep /usr/bin/mtr-packet' "$ctl/postinst" || { echo "postinst lacks setcap" >&2; exit 1; }
[ -x "$ctl/postinst" ] && [ -x "$ctl/postrm" ] || { echo "maintainer scripts not executable" >&2; exit 1; }
bash -n "$ctl/postinst" && bash -n "$ctl/postrm"
echo "deb check ok"
