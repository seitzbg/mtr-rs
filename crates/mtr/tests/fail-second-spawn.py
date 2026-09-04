#!/usr/bin/env python3
"""Helper wrapper for tests: behaves like fake-mtr-packet.py the first time it is
spawned and exits immediately every later time.

$MTR_FAKE_COUNTER names a file used as the invocation counter; each run appends one
byte to it and looks at the resulting size. An immediate exit makes the client's
startup handshake fail, which is the `Fatal::Abort` path for a second target.
"""
import os
import sys

counter = os.environ["MTR_FAKE_COUNTER"]
with open(counter, "ab") as f:
    f.write(b"x")
if os.path.getsize(counter) > 1:
    sys.exit(1)

fake = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fake-mtr-packet.py")
os.execv(sys.executable, [sys.executable, fake] + sys.argv[1:])
