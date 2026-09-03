#!/usr/bin/env python3
"""Fake mtr-packet for tests: deterministic replies, no privileges required.

ttl 1 -> ttl-expired from 10.0.0.1 (1 ms); ttl 2 -> ttl-expired from 10.0.0.2 (2 ms);
ttl >= 3 -> reply from the target (3 ms). Feature 'sctp' is unsupported, everything else ok.
"""
import sys


def main() -> None:
    out = sys.stdout
    for raw in sys.stdin:
        toks = raw.split()
        if len(toks) < 2 or len(toks) % 2 != 0:
            out.write("0 command-parse-error\n")
            out.flush()
            continue
        token, name = toks[0], toks[1]
        args = dict(zip(toks[2::2], toks[3::2]))
        if name == "check-support":
            feature = args.get("feature", "")
            if feature == "version":
                out.write(f"{token} feature-support support 0.96-fake\n")
            else:
                out.write(f"{token} feature-support support {'no' if feature == 'sctp' else 'ok'}\n")
        elif name == "send-probe":
            ttl = int(args.get("ttl", "255"))
            family = "ip-6" if "ip-6" in args else "ip-4"
            target = args.get(family, "0.0.0.0")
            if ttl < 3:
                out.write(f"{token} ttl-expired {family} 10.0.0.{ttl} round-trip-time {ttl * 1000}\n")
            else:
                out.write(f"{token} reply {family} {target} round-trip-time 3000\n")
        else:
            out.write(f"{token} unknown-command\n")
        out.flush()


if __name__ == "__main__":
    main()
