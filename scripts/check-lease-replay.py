#!/usr/bin/env python3
"""The lease baseline's receipts are reproducible by someone other than their author.

Copies docs/experiments/lease/synthetic/ into a fresh temporary root and runs
the same driver for each incumbent there, so its receipts land in the copy and
never in the repository. Then compares receipt SHAPES against the recorded
runs: the ordered sequence of record kinds, each command's shell line, and
each command's exit code. Output text and timing are allowed to differ; the
sequence and the exit codes are not.

A baseline whose receipts only its author produced is a claim; one a stranger
can reproduce is evidence. Exit 0 when both tools reproduce, 1 when either
differs, 3 when a replay could not run at all.

    check-lease-replay.py            replay both incumbents and compare
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
LEASE = REPO / "docs" / "experiments" / "lease"
TOOLS = ("process-compose", "pueue")


import re

def _mask(shell):
    """Process ids, ports and temporary roots differ on every run and prove
    nothing either way; the command, its flags and its exit code are the shape."""
    if not shell:
        return shell
    shell = re.sub(r"/(?:private/)?(?:tmp|var/folders)/\S+", "<tmp>", shell)
    return re.sub(r"\b\d{2,}\b", "#", shell)


def shape(transcript: Path):
    rows = [json.loads(l) for l in transcript.read_text().splitlines() if l.strip()]
    out = []
    for r in rows:
        if r.get("kind") == "command":
            out.append(("command", _mask(r.get("shell")), r.get("returncode")))
        else:
            out.append((r.get("kind"), None, None))
    return out


def main():
    for t in TOOLS:
        if not (LEASE / "runs" / t / "transcript.jsonl").is_file():
            print(f"FAIL no recorded transcript for {t}")
            return 1
    tmp = Path(tempfile.mkdtemp(prefix="lease-replay-"))
    try:
        copy = tmp / "lease"
        shutil.copytree(LEASE / "synthetic", copy / "synthetic")
        # The driver records its own decisions through `emit`; a replay is not a seat,
        # so give it a silent one rather than letting the replay fail on a missing shim.
        bindir = tmp / "bin"; bindir.mkdir()
        (bindir / "emit").write_text("#!/bin/sh\nexit 0\n"); (bindir / "emit").chmod(0o755)
        env = {**os.environ, "PATH": f"{bindir}:{os.environ.get('PATH','')}"}
        bad = 0
        for t in TOOLS:
            r = subprocess.run([sys.executable, str(copy / "synthetic" / "run.py"), t],
                               cwd=str(copy), env=env, capture_output=True, text=True, timeout=600)
            replay = copy / "runs" / t / "transcript.jsonl"
            if not replay.is_file():
                print(f"FAIL {t}: replay produced no transcript (exit {r.returncode}): {r.stderr.strip()[-200:]}")
                return 3
            want, got = shape(LEASE / "runs" / t / "transcript.jsonl"), shape(replay)
            if want == got:
                print(f"ok   {t}: {len(got)} receipts reproduce, same sequence and exit codes")
                continue
            bad += 1
            n = next((i for i, (a, b) in enumerate(zip(want, got)) if a != b), min(len(want), len(got)))
            print(f"FAIL {t}: receipts diverge at record {n+1} of {len(want)} recorded / {len(got)} replayed")
            print(f"     recorded: {want[n] if n < len(want) else '<end>'}")
            print(f"     replayed: {got[n] if n < len(got) else '<end>'}")
        return 1 if bad else 0
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
