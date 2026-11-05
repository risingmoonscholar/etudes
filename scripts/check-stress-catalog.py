#!/usr/bin/env python3
"""Verify that every tracked stress scenario has one actionable catalog row."""
import json
import argparse
import re
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
# With no journal key a tool uses the login keychain of whoever runs the suite,
# and can add a key to it. A scenario may drop the key only through
# keychain_fenced in stress/lib.sh, which is why that file alone may say this.
KEY_DROPPED = re.compile(
    r"\bunset\b[^\n#]*\bETUDE_JOURNAL_KEY\b"
    r"|\benv\b[^\n#]*(-u\s*|--unset[=\s])ETUDE_JOURNAL_KEY\b"
    r"|\benv\s+(-\w+\s+)*-i\b"
    r"|\benv\s+--ignore-environment\b"
    r"|\benv\s+(-\w+\s+)*-(\s|$)"
    r"|\bexec\s+(-\w+\s+)*-\w*c\w*(\s|$)"
    r"|\bexport\s+-n\b[^\n#]*\bETUDE_JOURNAL_KEY\b"
    r"|(\bpop|\bdel\b|\bremove)[^\n#]*\bETUDE_JOURNAL_KEY\b"
)
# The check must catch each way of dropping the key, and leave alone lines that
# keep it. A planted line that slips through fails the run.
for planted in (
    "unset ETUDE_JOURNAL_KEY",
    "env -u ETUDE_JOURNAL_KEY sweep",
    "env -i sweep",
    "env - sweep",
    "env -0 - sweep",
    "exec -c sweep",
    "exec -cl sweep",
):
    if not KEY_DROPPED.search(planted):
        errors.append(f"self-test: dropping the key by {planted!r} is not caught")
for kept in ("env FOO=1 sweep", "exec sweep status", "ls -c"):
    if KEY_DROPPED.search(kept):
        errors.append(f"self-test: {kept!r} is flagged but keeps the key")
for scenario in sorted((root / "stress/scenarios").glob("*.sh")):
    for number, line in enumerate(scenario.read_text().splitlines(), 1):
        if not line.lstrip().startswith("#") and KEY_DROPPED.search(line):
            errors.append(
                f"{scenario.name}:{number}: drops ETUDE_JOURNAL_KEY outside keychain_fenced; "
                "a run could reach the login keychain"
            )
if errors:
    print("FAIL stress catalog")
    print("\n".join("  " + error for error in errors))
    sys.exit(1)
counts = {tier: sum(row["tier"] == tier for row in catalog) for tier in ("fast", "load", "platform")}
print(f"ok stress catalog: {len(catalog)} scenarios; fast={counts['fast']} load={counts['load']} platform={counts['platform']}")
