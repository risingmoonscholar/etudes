#!/usr/bin/env python3
"""Check every stress scenario against the suite's rules for a good test case.

A rule here is a fact a script can check in the source. The rules that need a
run are in stress/mustfire (planted bugs) and scripts/check-stress-baseline.py
(floors and pinned unproven counts).

Exit 0 when every scenario follows every rule, 1 otherwise. Each failure names
the scenario, the line, the rule, and what to do instead.
"""
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent.parent
SCEN = ROOT / "stress/scenarios"

TERMINATE = re.compile(r"\bkill\s+(-9|-KILL|-TERM|-INT|-HUP|-QUIT|-s\s+(KILL|TERM|INT|HUP|QUIT))\b")
# A witnessed stop: the process is stopped and its stopped state is confirmed
# (or the tool's own watcher holds it), before the fault is injected.
WITNESS = re.compile(r"\bstop_confirmed\b|--watch-|\bpause_on_\w+|kill\s+-STOP\b")

# Scenarios that send a terminating signal for a reason other than a timing
# probe. Each entry says why; a new entry needs the same.
KILL_NOT_A_PROBE = {
    "70-content-blindness-traps": "stops a scan that blocked on a FIFO; the kill is the timeout, not the probe",
    "85-scenario-exit-status": "kills its own scenario shell to test the harness verdict",
}
# Scenarios whose subject is two runs at the same time. The overlap must be
# witnessed, not hoped for.
MUST_WITNESS_OVERLAP = {"30-concurrent-applies", "30-same-root-contention"}


def lines(path):
    return path.read_text(errors="replace").splitlines()


def rule_fake_home(errors):
    lib = (ROOT / "stress/lib.sh").read_text()
    if not re.search(r"^\s*export HOME=.*mktemp", lib, re.M):
        errors.append("stress/lib.sh: rule fake-home: export a throwaway HOME made with mktemp, "
                      "so no scenario runs with the real home folder")


def check_scenario(path, catalog_ids, errors):
    name = path.stem
    text = path.read_text(errors="replace")
    ls = lines(path)

    def err(n, rule, fix):
        errors.append(f"{path.relative_to(ROOT)}:{n}: rule {rule}: {fix}")

    for n, line in enumerate(ls, 1):
        if line.startswith("unproven "):
            err(n, "conditional-unproven", "an unconditional unproven is a note, not a result; "
                "it also turns off the floor. Print the note with echo, or test the thing")
        if re.search(r"ETUDE_JOURNAL_KEY\s*=\s*(['\"]?[0-9a-fA-F]{16,}['\"]?(\s|$|\))|'[0-9a-fA-F]+'\s*\*)", line):
            err(n, "harness-key", "use the random key from stress/lib.sh; do not set a fixed or global key")
        if (re.search(r"ETUDE_JOURNAL_KEY=(''|\"\")(\s|$)", line) or "env -u ETUDE_JOURNAL_KEY" in line) \
                and "keychain_fenced" not in line:
            err(n, "keychain-fence", "a run with an empty or missing key must go through keychain_fenced, "
                "so a regression cannot reach the login keychain")
        if re.search(r"(^|[\s\"'=(])/tmp/", line) and ":-/tmp" not in line:
            err(n, "own-scratch", "write only inside the scenario's workdir, never to a fixed /tmp path")
        if re.search(r"(^|[\s;&|(])cargo\s", line) and not line.lstrip().startswith("#"):
            err(n, "release-binary", "a scenario tests the release binaries in BIN; do not run cargo")
        if re.search(r"\bassert_exit\b.*--\s+(true|:)\s*$", line):
            err(n, "real-assertion", "this assertion runs a command that always succeeds; assert the real outcome")
        if TERMINATE.search(line) and n > 1:
            before = "\n".join(ls[max(0, n - 3):n - 1])
            if re.search(r"\bsleep\s+0?\.\d+", before):
                err(n, "no-sleep-and-hope", "a short sleep before a signal is a timing guess; "
                    "stop the process, confirm it is stopped, then signal")

    if TERMINATE.search(text) and name not in KILL_NOT_A_PROBE and not WITNESS.search(text):
        err(1, "pause-then-inject", "this scenario signals a process mid-run without a witnessed stop. "
            "Stop it (kill -STOP, then confirm state T, or journal-dump --watch-apply), check the "
            "state you want, then send the signal")
    if name in MUST_WITNESS_OVERLAP and "--watch-apply" not in text:
        err(1, "witnessed-overlap", "hold the first run with journal-dump --watch-apply, run the second "
            "to completion, then let the first continue")
    if name == "25-grace-window" and not re.search(r"touch\s+-a\b|os\.utime\([^)]*,\s*\(", text):
        err(1, "control-the-variable", "set the access time explicitly, so the check proves the window "
            "uses modification time and not access time")
    if name not in catalog_ids:
        err(1, "catalogued", "add a catalog row with the contract this scenario checks")


def rule_harness_rows(catalog, errors):
    for row in catalog:
        if row["id"] == "85-scenario-exit-status" and row.get("kind") != "harness":
            errors.append("stress/catalog.json: rule harness-kind: 85-scenario-exit-status tests the "
                          "harness, not a product promise; mark its row \"kind\": \"harness\"")
        if row.get("kind") not in (None, "harness"):
            errors.append(f"stress/catalog.json: {row['id']}: kind must be absent or \"harness\"")


def main():
    catalog = json.loads((ROOT / "stress/catalog.json").read_text())
    ids = {row["id"] for row in catalog}
    errors = []
    rule_fake_home(errors)
    for path in sorted(SCEN.glob("*.sh")):
        check_scenario(path, ids, errors)
    rule_harness_rows(catalog, errors)
    for e in errors:
        print("FAIL " + e)
    if errors:
        print(f"FAIL stress taste: {len(errors)} problem(s)")
        return 1
    print(f"ok stress taste: {len(list(SCEN.glob('*.sh')))} scenarios follow every rule")
    return 0


if __name__ == "__main__":
    sys.exit(main())
