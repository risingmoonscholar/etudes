#!/usr/bin/env python3
"""Run one owned process group to a monotonic deadline and write its evidence."""
import argparse
import json
import os
import signal
import subprocess
import time


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
    try:
        stdout, stderr = child.communicate(timeout=args.timeout_ms / 1000)
    except subprocess.TimeoutExpired:
        timed_out = True
        os.killpg(child.pid, signal.SIGKILL)
        stdout, stderr = child.communicate()
    elapsed_ms = (time.monotonic_ns() - started) // 1_000_000
    result = {
        "command": args.command[1:],
        "duration_ms": elapsed_ms,
        "exit": child.returncode,
        "signal": -child.returncode if child.returncode < 0 else None,
        "timed_out": timed_out,
        "stdout": stdout.decode("utf-8", "replace"),
        "stderr": stderr.decode("utf-8", "replace"),
    }
    with open(args.evidence, "w") as output:
        json.dump(result, output, indent=2, sort_keys=True)
        output.write("\n")
    return 124 if timed_out else child.returncode


if __name__ == "__main__":
    raise SystemExit(main())
