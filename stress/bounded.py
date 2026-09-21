#!/usr/bin/env python3
"""Run one owned process group to a monotonic deadline and always reap it."""
import argparse
import json
import os
import signal
import subprocess
import time


class Interrupted(Exception):
    """The supervisor received a signal and must clean up its owned group."""


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--timeout-ms", type=int, required=True)
    parser.add_argument("--evidence", required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.timeout_ms <= 0 or not args.command or args.command[0] != "--":
        parser.error("use --timeout-ms N --evidence FILE -- COMMAND ...")

    started = time.monotonic_ns()
    child = subprocess.Popen(
        args.command[1:], stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        start_new_session=True,
    )
    timed_out = False
    interrupted_by = None
    stdout = b""
    stderr = b""

    def interrupt(signum, _frame):
        nonlocal interrupted_by
        interrupted_by = signum
        raise Interrupted()

    old_handlers = {
        signum: signal.signal(signum, interrupt)
        for signum in (signal.SIGHUP, signal.SIGINT, signal.SIGQUIT, signal.SIGTERM)
    }
    try:
        try:
            stdout, stderr = child.communicate(timeout=args.timeout_ms / 1000)
        except subprocess.TimeoutExpired:
            timed_out = True
            os.killpg(child.pid, signal.SIGKILL)
            stdout, stderr = child.communicate()
        except Interrupted:
            # The supervisor owns this session. Do not leave it running when
            # its caller stops; kill the whole group, then wait for its leader.
            os.killpg(child.pid, signal.SIGKILL)
            stdout, stderr = child.communicate()
    finally:
        for signum, handler in old_handlers.items():
            signal.signal(signum, handler)
        # This is deliberately unconditional: success, timeout, launch-side
        # interruption, and future exception paths all end by reaping the
        # direct child. SIGKILL is the one signal no program can handle.
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGKILL)
            stdout, stderr = child.communicate()

    elapsed_ms = (time.monotonic_ns() - started) // 1_000_000
    result = {
        "command": args.command[1:],
        "duration_ms": elapsed_ms,
        "exit": child.returncode,
        "signal": -child.returncode if child.returncode < 0 else None,
        "timed_out": timed_out,
        "interrupted_by": interrupted_by,
        "reaped": child.poll() is not None,
        "stdout": stdout.decode("utf-8", "replace"),
        "stderr": stderr.decode("utf-8", "replace"),
    }
    with open(args.evidence, "w") as output:
        json.dump(result, output, indent=2, sort_keys=True)
        output.write("\n")
    if timed_out:
        return 124
    if interrupted_by is not None:
        return 128 + interrupted_by
    return child.returncode


if __name__ == "__main__":
    raise SystemExit(main())
