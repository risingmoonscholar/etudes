#!/usr/bin/env python3
"""Check structured stress results against the committed, nondecreasing baseline."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent


def check(baseline, catalog, summary, previous=None, pins=None):
    errors = []
    pins = {} if pins is None else pins
    if previous is not None:
        for name, floor in previous.items():
            if name not in baseline or baseline[name] < floor:
                errors.append(f"{name}: baseline entry removed or lowered (previous {floor})")
    present = {row["id"] for row in catalog}
    present &= {path.stem for path in (ROOT / "stress/scenarios").glob("*.sh")}
    for name in present - baseline.keys():
        errors.append(f"{name}: missing baseline entry")
    for name in baseline:
        if type(baseline[name]) is not int or baseline[name] < 1:
            errors.append(f"{name}: assertion floor must be a positive integer")
        if name not in present:
            errors.append(f"{name}: baseline scenario disappeared")
    for row in summary["cases"]:
        name = row["id"]
        if name not in baseline:
            errors.append(f"{name}: missing baseline entry")
        # A host that cannot prove any assertion has no comparable result.
        # Partial runs still meet both the floor and the exact unproven pin.
        if row["passed"] == 0 and row["unproven"] > 0:
            continue
        if name in baseline and row["passed"] < baseline[name]:
            errors.append(f"{name}: {row['passed']} passing assertions below baseline {baseline[name]}")
        expected = pins.get(name, 0)
        if row["unproven"] != expected:
            errors.append(f"{name}: {row['unproven']} unproven results differs from pin {expected}")
    for name, count in pins.items():
        if name not in present:
            errors.append(f"{name}: unproven pin names an absent scenario")
        if type(count) is not int or count < 1:
            errors.append(f"{name}: unproven pin must be a positive integer")
    covered = {row["id"] for row in summary["cases"] + summary["not_run"]}
    for name in present - covered:
        errors.append(f"{name}: missing from both results and not_run")
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("summary", type=Path)
    parser.add_argument("--base", default="origin/main")
    args = parser.parse_args()
    path = "stress/baseline.json"
    baseline = json.loads((ROOT / path).read_text())
    errors = []
    # HEAD also protects an uncommitted edit after the branch acquired a baseline.
    for ref in dict.fromkeys((args.base, "HEAD")):
        exists = subprocess.run(["git", "cat-file", "-e", f"{ref}:{path}"],
                                cwd=ROOT, capture_output=True)
        if exists.returncode:
            valid_ref = subprocess.run(["git", "rev-parse", "--verify", ref],
                                       cwd=ROOT, capture_output=True)
            if valid_ref.returncode:
                errors.append(f"cannot resolve baseline comparison ref {ref}")
            continue  # Initial introduction: this ref has no baseline yet.
        previous = json.loads(subprocess.check_output(
            ["git", "show", f"{ref}:{path}"], cwd=ROOT, text=True))
        for name, floor in previous.items():
            if name not in baseline or baseline[name] < floor:
                errors.append(f"{name}: baseline entry removed or lowered relative to {ref}")
    errors += check(baseline, json.loads((ROOT / "stress/catalog.json").read_text()),
                    json.loads(args.summary.read_text()),
                    pins=json.loads((ROOT / "stress/unproven.json").read_text()))
    for error in errors:
        print("FAIL baseline: " + error)
    if errors:
        return 1
    print(f"ok stress baseline: {len(baseline)} scenarios retained; structured assertion floors met")
    return 0


if __name__ == "__main__":
    sys.exit(main())
