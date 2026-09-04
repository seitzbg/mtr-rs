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
# `cargo deb --no-build` warns "... will not be built" for each asset outside target/release
# (the man pages, completions and the example config): it cannot know they were produced by
# `cargo xtask dist` just above rather than by a cargo build. The warnings are expected.
deb=$(cargo deb -p mtr --no-build | tail -1)
[ -f "$deb" ] || { echo "cargo deb produced no file" >&2; exit 1; }
echo "built $deb"

contents=$(dpkg-deb -c "$deb")
for path in ./usr/bin/mtr-rs ./usr/bin/mtr-rs-packet ./usr/share/man/man8/mtr-rs.8.gz \
            ./usr/share/man/man8/mtr-rs-packet.8.gz ./usr/share/bash-completion/completions/mtr-rs \
            ./usr/share/zsh/site-functions/_mtr-rs ./usr/share/fish/vendor_completions.d/mtr-rs.fish \
            ./usr/share/doc/mtr-rs/config.example.toml; do
  grep -q " $path$" <<<"$contents" || { echo "missing from package: $path" >&2; exit 1; }
done
grep -qE '^-rwxr-xr-x .* \./usr/bin/mtr-rs-packet$' <<<"$contents" || { echo "mtr-rs-packet is not 755" >&2; exit 1; }
for path in ./usr/share/man/man8/mtr-rs.8.gz ./usr/share/man/man8/mtr-rs-packet.8.gz \
            ./usr/share/bash-completion/completions/mtr-rs ./usr/share/zsh/site-functions/_mtr-rs \
            ./usr/share/fish/vendor_completions.d/mtr-rs.fish; do
  grep -qE "^-rw-r--r-- .* $path\$" <<<"$contents" || { echo "not 644 in package: $path" >&2; exit 1; }
done

info=$(dpkg-deb -I "$deb")
grep -q '^ Package: mtr-rs$' <<<"$info" || { echo "package name is not mtr-rs" >&2; exit 1; }
# The package installs alongside the distribution's mtr / mtr-tiny: it owns only its own
# /usr/bin/mtr-rs* paths, so it must declare no relationship with them at all.
for field in Conflicts Provides Replaces; do
  if grep -q "^ $field:" <<<"$info"; then echo "$field must be absent" >&2; exit 1; fi
done
grep -q '^ Homepage: https://github.com/seitzbg/mtr-rs$' <<<"$info" || { echo "Homepage missing" >&2; exit 1; }
grep -q '^ Section: net$' <<<"$info" || { echo "Section is not net" >&2; exit 1; }
grep -qE '^ Depends: .*libc6' <<<"$info" || { echo "Depends not auto-computed" >&2; exit 1; }
grep -qE '^ Depends: .*libcap2-bin' <<<"$info" || { echo "Depends lacks libcap2-bin (postinst needs setcap)" >&2; exit 1; }
grep -q '^ Priority: optional$' <<<"$info" || { echo "Priority is not optional" >&2; exit 1; }
grep -qE '^ Maintainer: .+$' <<<"$info" || { echo "Maintainer missing" >&2; exit 1; }

ctl=$(mktemp -d); trap 'rm -rf "$ctl"' EXIT
dpkg-deb -e "$deb" "$ctl"
grep -q 'setcap cap_net_raw+ep /usr/bin/mtr-rs-packet' "$ctl/postinst" || { echo "postinst lacks setcap" >&2; exit 1; }
[ -x "$ctl/postinst" ] && [ -x "$ctl/postrm" ] || { echo "maintainer scripts not executable" >&2; exit 1; }
sh -n "$ctl/postinst" && sh -n "$ctl/postrm"
echo "deb check ok"
