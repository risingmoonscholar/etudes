#!/usr/bin/env python3
"""Check what one tool run wrote against the shared progress schema.

    check-progress-records.py --stderr FILE --stdout FILE --operation NAME
        [--killed] [--planned N] [--completed N] [--journalled N] [--forbid TEXT]

Everything on stderr that starts with `{` must be a whole record: a kill or an
extractor sharing the stream must not be able to tear one. Other lines are the
tools' ordinary diagnostics. With --killed there must be no terminal record, and
the records present must still be well formed.
"""
import argparse
import json
import sys
from pathlib import Path

FIELDS = {
    "schema_version", "event", "tool_version", "operation_id", "operation",
    "phase", "planned", "completed", "journalled",
}
MAX_EVENTS = 12


def fail(message):
    print(f"FAIL progress records: {message}", file=sys.stderr)
    sys.exit(1)


def records(text):
    rows = []
    for line in text.splitlines():
        if not line.startswith("{"):
            continue
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            fail(f"torn record: {line!r}")
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stderr", type=Path, required=True)
    parser.add_argument("--stdout", type=Path)
    parser.add_argument("--operation", required=True)
    parser.add_argument("--killed", action="store_true")
    parser.add_argument("--planned", type=int)
    parser.add_argument("--completed", type=int)
    parser.add_argument("--journalled", type=int)
    parser.add_argument("--forbid", action="append", default=[])
    args = parser.parse_args()

    stderr = args.stderr.read_text(errors="replace")
    rows = records(stderr)
    if not rows:
        fail("no records")
    if len(rows) > MAX_EVENTS:
        fail(f"{len(rows)} records exceeds the bound of {MAX_EVENTS}")
    for text in args.forbid:
        if any(text in json.dumps(row) for row in rows):
            fail("a record disclosed a forbidden value")

    for row in rows:
        if set(row) != FIELDS:
            fail(f"fields differ from schema 1: {sorted(row)}")
        if row["schema_version"] != 1 or row["event"] != "progress":
            fail(f"unexpected schema or event: {row}")
        if row["operation"] != args.operation:
            fail(f"operation {row['operation']!r}, expected {args.operation!r}")
        for name in ("planned", "completed", "journalled"):
            if type(row[name]) is not int or row[name] < 0:
                fail(f"{name} is not a count: {row}")
        if row["phase"] not in {"planning", "advancing", "done", "error"}:
            fail(f"unknown phase: {row}")
    if len({row["operation_id"] for row in rows}) != 1:
        fail("records carry more than one operation id")

    terminal = [row for row in rows if row["phase"] in {"done", "error"}]
    if args.killed:
        if terminal:
            fail("a killed run wrote a terminal record")
    else:
        if len(terminal) != 1 or rows[-1] is not terminal[0]:
            fail(f"expected exactly one terminal record, last: {rows[-1]}")

    total = 0
    last_completed = last_journalled = 0
    for index, row in enumerate(rows):
        if row["phase"] == "planning":
            # Zero means not known yet: only ever before a count, never after.
            if row["planned"] == 0 and (index != 0 or row["completed"]):
                fail(f"indeterminate record out of place: {row}")
            if row["planned"]:
                if total:
                    fail("the count was announced twice")
                total = row["planned"]
        elif row["phase"] == "advancing" and not total:
            fail(f"advancing before a count: {row}")
        if total and row["planned"] != total:
            fail(f"planned changed from {total}: {row}")
        if row["completed"] > row["planned"]:
            fail(f"completed exceeds planned: {row}")
        if row["journalled"] > row["completed"]:
            fail(f"journalled exceeds completed: {row}")
        if row["completed"] < last_completed or row["journalled"] < last_journalled:
            fail(f"a count went backwards: {row}")
        last_completed, last_journalled = row["completed"], row["journalled"]

    last = rows[-1]
    for name, wanted in (("planned", args.planned), ("completed", args.completed),
                         ("journalled", args.journalled)):
        if wanted is not None and last[name] != wanted:
            fail(f"last record has {name}={last[name]}, expected {wanted}: {last}")

    if args.stdout:
        stdout = args.stdout.read_text(errors="replace")
        if '"event":"progress"' in stdout:
            fail("progress reached stdout")
        if stdout.startswith("{") and "--killed" not in sys.argv:
            result = json.loads(stdout)
            if result["operation_id"] != last["operation_id"]:
                fail("records and the result disagree on the operation id")
            if result["tool_version"] != last["tool_version"]:
                fail("records and the result disagree on the tool version")
            finished = result["status"] in {"done", "nothing_to_do"}
            if (last["phase"] == "done") != finished:
                fail(f"phase {last['phase']} beside result status {result['status']}")
    print(f"ok progress records: {args.operation}, {len(rows)} records, last {last['phase']} "
          f"{last['completed']}/{last['planned']}")


main()
