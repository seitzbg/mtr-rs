#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-only
# Run one of mtr's upstream helper test suites against $MTR_PACKET.
# usage: tests/compat/run.sh <cmdparse|param|probe> [unittest args...]
#        tests/compat/run.sh --compare [suite...]      # ours vs the C baseline
#        tests/compat/run.sh --report-only [suite...]  # same, but never fails
#        tests/compat/run.sh --self-test               # check the output parser
# options: --listen-seq N (or $MTR_LISTEN_SEQUENCE) rebuilds test/mtr-packet-listen
#          with -DSEQUENCE_NUM=N, see the param.py note in the known-divergence list
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
repo=${MTR_C_REPO:-$HOME/git/mtr}
baseline=${MTR_BASELINE:-/usr/bin/mtr-packet}
report_only=0

# Failures we understand and accept: "<unittest test id>|<reason>". They are printed
# as "known: <id> -- <reason>" and never make --compare fail.
listen_seq_reason="test/packet_listen.c hard-codes SEQUENCE_NUM 33000 while upstream commit e95eaf4 moved MIN_PORT to 33434; our helper (and the C 0.96 helper) start at 33434, so the listener times out. Rerun with --listen-seq 33434 (needs its own setcap) to verify positively"
known_divergences=(
    "__main__.TestProbeICMPv4.test_exhaust_probes|sends 4096 probes; mtr 0.96 (7b01773) raised MAX_PROBES to 10240 so neither our helper nor C 0.96 can exhaust the table; the installed 0.95 baseline (MAX_PROBES 1024) passes"
    "__main__.TestParameters.test_size|$listen_seq_reason"
    "__main__.TestParameters.test_pattern|$listen_seq_reason"
    "__main__.TestParameters.test_tos|$listen_seq_reason"
    "__main__.TestIPv6Parameters.test_param|$listen_seq_reason"
)

known_reason() {
    local entry
    for entry in "${known_divergences[@]}"; do
        case "$entry" in "$1|"*) printf '%s\n' "${entry#*|}"; return 0;; esac
    done
    return 1
}

die() { echo "$1" >&2; exit "$2"; }

# We run the suites from the C repo's test/ directory, so a relative helper path
# given on the command line has to be pinned to the caller's cwd first.
abspath() { case $1 in /*) printf '%s\n' "$1";; *) printf '%s/%s\n' "$(cd "$(dirname "$1")" && pwd)" "$(basename "$1")";; esac; }
if [ -n "${MTR_PACKET:-}" ] && [ -e "$MTR_PACKET" ]; then MTR_PACKET=$(abspath "$MTR_PACKET"); export MTR_PACKET; fi
if [ -e "$baseline" ]; then baseline=$(abspath "$baseline"); fi

# The param suite drives test/mtr-packet-listen, which needs cap_net_raw of its
# own; build it on demand and report whether it is actually usable.
listen_ready() {
    local src="$repo/test/packet_listen.c"
    if [ -n "${MTR_LISTEN_SEQUENCE:-}" ]; then
        # Build the listener for a different first sequence number without touching
        # the C repo's source: patch a scratch copy under our own target/ directory.
        mkdir -p "$root/target/compat"
        src="$root/target/compat/packet_listen-$MTR_LISTEN_SEQUENCE.c"
        sed "s/^#define SEQUENCE_NUM .*/#define SEQUENCE_NUM $MTR_LISTEN_SEQUENCE/" \
            "$repo/test/packet_listen.c" > "$src"
        rm -f "$repo/test/mtr-packet-listen"
    fi
    if [ ! -x "$repo/test/mtr-packet-listen" ]; then
        cc -I"$repo" -o "$repo/test/mtr-packet-listen" "$src" >&2
        echo "built $repo/test/mtr-packet-listen from $src -- it needs cap_net_raw: sudo setcap cap_net_raw+ep $repo/test/mtr-packet-listen" >&2
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

# Run a suite and capture its combined output. A file, not $(...): param.py can leave
# an orphaned test/mtr-packet-listen behind (it never sees the sequence it waits for),
# and an orphan holding the write end of a command-substitution pipe would hang the
# wrapper forever. A file descriptor on a temp file cannot block us.
capture_suite() {
    local out; out=$(mktemp "${TMPDIR:-/tmp}/mtr-compat.XXXXXX")
    run_suite "$@" > "$out" 2>&1 || true
    cat "$out"
    rm -f "$out"
}

# Turn verbose unittest output into "<test id> <status>" lines. The parenthesised
# text is the class alone on Python <= 3.10 ("test_x (__main__.C) ... ok") and the
# full id on Python >= 3.11 ("test_x (__main__.C.test_x) ... ok"), so we always
# rebuild "<class>.<method>" from the leading method name; comparing by class only
# would let an ours-only failure hide behind a baseline failure in the same class.
parse_results() {
    awk '
        /^[A-Za-z_][A-Za-z0-9_]* \(.*\)/ {
            method = $1
            p = index($0, "("); rest = substr($0, p + 1)
            q = index(rest, ")"); id = substr(rest, 1, q - 1)
            sub(/ .*/, "", id)
            suffix = "." method
            if (length(id) > length(suffix) && substr(id, length(id) - length(suffix) + 1) == suffix)
                id = substr(id, 1, length(id) - length(suffix))
            id = id "." method
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
        ours_out=$(export MTR_PACKET; capture_suite "$suite" "${only[@]}" -v)
        if [ -x "$baseline" ]; then
            base_out=$(export MTR_PACKET="$baseline"; capture_suite "$suite" "${only[@]}" -v)
            desc="baseline: $(printf '%s\n' "$base_out" | count_status ok) passed $(printf '%s\n' "$base_out" | failed_ids | wc -l) failed $(printf '%s\n' "$base_out" | count_status skipped) skipped"
        else
            base_out=""
            desc="baseline: n/a ($baseline missing)"
        fi
        local real_only=() known_lines="" id reason
        while read -r id; do
            [ -n "$id" ] || continue
            if reason=$(known_reason "$id"); then
                known_lines+="$(printf '          known: %s -- %s' "$id" "$reason")"$'\n'
            else
                real_only+=("$id")
            fi
        done < <(comm -23 <(printf '%s\n' "$ours_out" | failed_ids) \
                          <(printf '%s\n' "$base_out" | failed_ids))
        if [ ${#real_only[@]} -gt 0 ]; then ours_only="${real_only[*]}"; rc=1; else ours_only="none"; fi
        summary+="$(printf '%-9s ours: %s passed %s failed %s skipped   %s   ours-only failures: %s' \
            "$suite" "$(printf '%s\n' "$ours_out" | count_status ok)" \
            "$(printf '%s\n' "$ours_out" | failed_ids | wc -l)" \
            "$(printf '%s\n' "$ours_out" | count_status skipped)" "$desc" "$ours_only")"$'\n'
        summary+="$known_lines"
    done
    printf '\n=== compat summary (ours=%s baseline=%s) ===\n%s' "$MTR_PACKET" "$baseline" "$summary"
    # Report-only mode (CI without a C-helper baseline): print everything, fail nothing.
    [ "$report_only" = 0 ] || return 0
    return $rc
}

# Check that both unittest verbose line shapes parse to the same full test id.
self_test() {
    local got want='__main__.C.test_x FAIL
__main__.C.test_x ok'
    got=$(printf '%s\n' \
        "test_x (__main__.C) ... FAIL" \
        "test_x (__main__.C.test_x)" \
        "the docstring ... ok" | parse_results)
    if [ "$got" = "$want" ]; then
        echo "self-test ok"
    else
        printf 'self-test FAILED\nwant:\n%s\ngot:\n%s\n' "$want" "$got" >&2
        return 1
    fi
}

if [ "${1:-}" = "--listen-seq" ]; then MTR_LISTEN_SEQUENCE=${2:?--listen-seq needs a number}; shift 2; fi

case "${1:?suite name or --compare}" in
    --self-test) self_test ;;
    --compare) shift; compare "$@" ;;
    --report-only) shift; report_only=1; compare "$@" ;;
    *) run_suite "$@" ;;
esac
