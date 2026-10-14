#!/usr/bin/env python3
"""Observe a real copy; freeze only after seeing a nonempty partial destination.

Start before apply so interpreter startup cannot consume the copy window. The
FIFO supplies the exact apply PID once the watcher is ready. No sweep hook and
no change to the copy itself: all observations are OS stat/ps and real signals.
"""

import os
from pathlib import Path
import signal
import subprocess
import sys
import time


fifo, ready, destination, expected_size = sys.argv[1:]
expected_size = int(expected_size)
channel = os.open(fifo, os.O_RDWR)
Path(ready).touch()
pid = int(os.read(channel, 64).strip())
os.close(channel)
started = time.monotonic()
previous = started
intervals = []
first_nonempty = None
observed_size = 0
stop_requested = None
confirmed = False
state = "unknown"

while time.monotonic() - started < 5:
    now = time.monotonic()
    intervals.append(now - previous)
    previous = now
    try:
        os.kill(pid, 0)
        observed_size = os.stat(destination).st_size
    except ProcessLookupError:
        break
    except FileNotFoundError:
        time.sleep(0.0001)
        continue
    if observed_size > 0 and first_nonempty is None:
        first_nonempty = now
    if 0 < observed_size < expected_size:
        try:
            os.kill(pid, signal.SIGSTOP)
        except ProcessLookupError:
            break
        stop_requested = time.monotonic()
        for _ in range(100):
            result = subprocess.run(
                ["ps", "-p", str(pid), "-o", "state="],
                capture_output=True, text=True, check=False,
            )
            state = result.stdout.strip()
            if state.startswith("T"):
                confirmed = True
                break
            if not state:
                break
            time.sleep(0.001)
        if not confirmed:
            try:
                os.kill(pid, signal.SIGCONT)
            except ProcessLookupError:
                pass
        break
    if observed_size >= expected_size:
        break
    time.sleep(0.0001)

finished = time.monotonic()
# First line is shell-readable; remaining lines are diagnostic, not assertions.
print(int(confirmed), state or "unknown")
mean_poll_ms = 1000 * sum(intervals[1:]) / max(1, len(intervals) - 1)
max_poll_ms = 1000 * max(intervals)
stop_ms = "n/a" if stop_requested is None else f"{1000 * (finished - stop_requested):.3f}"
partial_to_confirm_ms = "n/a" if first_nonempty is None else f"{1000 * (finished - first_nonempty):.3f}"
print(
    f"    timing   cross-volume: size={expected_size // (1024 * 1024)}MiB "
    f"polls={len(intervals)} poll_mean_ms={mean_poll_ms:.3f} "
    f"poll_max_ms={max_poll_ms:.3f} stop_confirm_ms={stop_ms} "
    f"partial_to_confirm_ms={partial_to_confirm_ms} observed_destination={observed_size}/{expected_size} "
    f"stopped={int(confirmed)}"
)
