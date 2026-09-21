#!/usr/bin/env python3
"""Verify that every tracked stress scenario has one actionable catalog row."""
import json
import argparse
import subprocess
import sys
from pathlib import Path

root = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--catalog", type=Path, default=root / "stress/catalog.json")
args = parser.parse_args()
catalog = json.loads(args.catalog.read_text())
tracked = {
    Path(path).stem
    for path in subprocess.check_output(
        ["git", "ls-files", "stress/scenarios/*.sh"], cwd=root, text=True
    ).splitlines()
}
seen = set()
errors = []
for row in catalog:
    required = ("id", "tier", "capability", "contract", "disposition")
    if any(not isinstance(row.get(key), str) or not row[key].strip() for key in required):
        errors.append(f"invalid catalog row: {row!r}")
        continue
    if row["id"] in seen:
        errors.append(f"duplicate scenario id: {row['id']}")
    seen.add(row["id"])
    if row["tier"] not in {"fast", "load", "platform"}:
        errors.append(f"{row['id']}: invalid tier {row['tier']!r}")
    if row["disposition"] not in {"keep", "repair", "merge", "split", "retire"}:
        errors.append(f"{row['id']}: invalid disposition {row['disposition']!r}")
if tracked - seen:
    errors.append("missing catalog rows: " + ", ".join(sorted(tracked - seen)))
if seen - tracked:
    errors.append("catalog rows without tracked scenario: " + ", ".join(sorted(seen - tracked)))
if errors:
    print("FAIL stress catalog")
    print("\n".join("  " + error for error in errors))
    sys.exit(1)
counts = {tier: sum(row["tier"] == tier for row in catalog) for tier in ("fast", "load", "platform")}
print(f"ok stress catalog: {len(catalog)} scenarios; fast={counts['fast']} load={counts['load']} platform={counts['platform']}")
