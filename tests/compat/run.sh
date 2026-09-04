#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-only
# Run one of mtr's upstream helper test suites against $MTR_PACKET.
# usage: tests/compat/run.sh <cmdparse|param|probe> [unittest args...]
#        tests/compat/run.sh --compare [suite...]   # ours vs the C baseline
set -euo pipefail

repo=${MTR_C_REPO:-$HOME/git/mtr}
baseline=${MTR_BASELINE:-/usr/bin/mtr-packet}

die() { echo "$1" >&2; exit "$2"; }

# We run the suites from the C repo's test/ directory, so a relative helper path
# given on the command line has to be pinned to the caller's cwd first.
abspath() { case $1 in /*) printf '%s\n' "$1";; *) printf '%s/%s\n' "$(cd "$(dirname "$1")" && pwd)" "$(basename "$1")";; esac; }
if [ -n "${MTR_PACKET:-}" ] && [ -e "$MTR_PACKET" ]; then MTR_PACKET=$(abspath "$MTR_PACKET"); export MTR_PACKET; fi
if [ -e "$baseline" ]; then baseline=$(abspath "$baseline"); fi

# The param suite drives test/mtr-packet-listen, which needs cap_net_raw of its
# own; build it on demand and report whether it is actually usable.
listen_ready() {
    if [ ! -x "$repo/test/mtr-packet-listen" ]; then
        cc -I"$repo" -o "$repo/test/mtr-packet-listen" "$repo/test/packet_listen.c" >&2
        echo "built $repo/test/mtr-packet-listen -- it needs cap_net_raw: sudo setcap cap_net_raw+ep $repo/test/mtr-packet-listen" >&2
    fi
    if [ "$(id -u)" = 0 ]; then return 0; fi
    command -v getcap >/dev/null 2>&1 || command -v /usr/sbin/getcap >/dev/null 2>&1 || return 1
    local getcap=getcap
    command -v getcap >/dev/null 2>&1 || getcap=/usr/sbin/getcap
    "$getcap" "$repo/test/mtr-packet-listen" 2>/dev/null | grep -q cap_net_raw
}

run_suite() {
    local suite=$1; shift
    case "$suite" in cmdparse|param|probe) ;; *) die "unknown suite: $suite" 2;; esac
    [ -d "$repo/test" ] || die "C repo not found at $repo (set MTR_C_REPO)" 3
    : "${MTR_PACKET:?set MTR_PACKET to the helper under test}"
    if [ "$suite" = param ] && ! listen_ready; then
        echo "skipping param.py: $repo/test/mtr-packet-listen lacks cap_net_raw" >&2
        return 0
    fi
    ( cd "$repo/test" && exec python3 "$suite.py" "$@" )
}

# Turn verbose unittest output into "<test id> <status>" lines.
parse_results() {
    awk '
        /^[A-Za-z_][A-Za-z0-9_]* \(.*\)/ {
            p = index($0, "("); rest = substr($0, p + 1)
            q = index(rest, ")"); id = substr(rest, 1, q - 1)
            sub(/ .*/, "", id)
        }
        /\.\.\. / {
            line = $0; sub(/.*\.\.\. /, "", line); split(line, a, " ")
            if (id != "") { print id " " a[1]; id = "" }
        }
    '
}

failed_ids() { parse_results | awk '$2 == "FAIL" || $2 == "ERROR" { print $1 }' | sort -u; }
count_status() { parse_results | awk -v want="$1" '$2 == want' | wc -l; }

compare() {
    local suites=("$@") rc=0 summary=""
    : "${MTR_PACKET:?set MTR_PACKET to the helper under test}"
    [ ${#suites[@]} -gt 0 ] || suites=(cmdparse param probe)
    [ -d "$repo/test" ] || die "C repo not found at $repo (set MTR_C_REPO)" 3
    for suite in "${suites[@]}"; do
        local ours_out base_out ours_only desc
        if [ "$suite" = param ] && ! listen_ready; then
            summary+="$(printf '%-9s SKIPPED: %s/test/mtr-packet-listen lacks cap_net_raw' "$suite" "$repo")"$'\n'
            continue
        fi
        # cmdparse.py's other class drives the C *client* binary, which this
        # workspace does not build; the helper class is the acceptance criterion.
        local only=()
        if [ "$suite" = cmdparse ]; then only=(TestCommandParse); fi
        ours_out=$( (export MTR_PACKET; run_suite "$suite" "${only[@]}" -v) 2>&1 || true)
        if [ -x "$baseline" ]; then
            base_out=$( (export MTR_PACKET="$baseline"; run_suite "$suite" "${only[@]}" -v) 2>&1 || true)
            desc="baseline: $(printf '%s\n' "$base_out" | count_status ok) passed $(printf '%s\n' "$base_out" | failed_ids | wc -l) failed"
        else
            base_out=""
            desc="baseline: n/a ($baseline missing)"
        fi
        ours_only=$(comm -23 <(printf '%s\n' "$ours_out" | failed_ids) \
                             <(printf '%s\n' "$base_out" | failed_ids) | paste -sd' ' -)
        [ -n "$ours_only" ] || ours_only="none"
        [ "$ours_only" = none ] || rc=1
        summary+="$(printf '%-9s ours: %s passed %s failed   %s   ours-only failures: %s' \
            "$suite" "$(printf '%s\n' "$ours_out" | count_status ok)" \
            "$(printf '%s\n' "$ours_out" | failed_ids | wc -l)" "$desc" "$ours_only")"$'\n'
    done
    printf '\n=== compat summary (ours=%s baseline=%s) ===\n%s' "$MTR_PACKET" "$baseline" "$summary"
    return $rc
}

case "${1:?suite name or --compare}" in
    --compare) shift; compare "$@" ;;
    *) run_suite "$@" ;;
esac
