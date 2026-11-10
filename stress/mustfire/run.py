#!/usr/bin/env python3
"""Run each stress scenario against its planted bugs. Every planted bug must be caught.

    BIN=target/release python3 stress/mustfire/run.py            every case
    BIN=target/release python3 stress/mustfire/run.py 60-full     cases whose scenario name contains "60-full"

A case is caught only when its scenario reports FAIL: exit 1 and at least one
FAIL line. A pass, an unproven result, a timeout or any other exit is a miss.
A miss means the scenario cannot see a bug that a real defect could cause.

Exit: 0 every case caught · 1 at least one miss · 2 bad input.

Each scenario runs from a scratch folder outside the repository, with the
harness's own random journal key and throwaway state folder (stress/lib.sh).
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
TIMEOUT = int(os.environ.get("MUSTFIRE_CASE_TIMEOUT_S", "600"))
SHIM = '#!/bin/sh\nexec python3 -I "{py}" {tool} "$@"\n'


def load_cases():
    cases = json.loads((HERE / "cases.json").read_text())
    sys.path.insert(0, str(HERE))
    import mutant  # noqa: E402  (the planted-bug table)
    errors = []
    scenarios = {p.stem for p in (ROOT / "stress/scenarios").glob("*.sh")}
    for c in cases:
        if c["scenario"] not in scenarios:
            errors.append(f"{c['scenario']}: no such scenario")
        if c["mutant"] not in mutant.MUTANTS:
            errors.append(f"{c['scenario']}: no planted bug named {c['mutant']}")
        if not c.get("bug"):
            errors.append(f"{c['scenario']}: {c['mutant']} has no plain description of the bug")
    return cases, errors


def mutant_bin(real, scratch):
    mb = Path(scratch) / "bin"
    mb.mkdir()
    for entry in Path(real).iterdir():
        if entry.is_file() and os.access(entry, os.X_OK):
            (mb / entry.name).symlink_to(entry)
    for tool in ("sweep", "stash", "unpack"):
        shim = mb / tool
        if shim.is_symlink():
            shim.unlink()
        shim.write_text(SHIM.format(py=HERE / "mutant.py", tool=tool))
        shim.chmod(0o755)
    return mb


def run_case(case, real, scratch):
    mb = mutant_bin(real, scratch)
    env = dict(os.environ, BIN=str(mb), MUSTFIRE_REAL_BIN=str(real),
               MUSTFIRE_MUTANT=case["mutant"], SCENARIO=case["scenario"])
    env.pop("ETUDE_JOURNAL_KEY", None)  # lib.sh makes a fresh random key
    script = ROOT / "stress/scenarios" / f"{case['scenario']}.sh"
    try:
        p = subprocess.run(["bash", str(script)], cwd=scratch, env=env,
                           stdin=subprocess.DEVNULL, capture_output=True, text=True,
                           errors="replace", timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        return "missed", f"timed out after {TIMEOUT}s"
    out = p.stdout + p.stderr
    fails = [l.strip() for l in out.splitlines() if l.startswith("    FAIL ")]
    if p.returncode == 1 and fails:
        return "caught", fails[0][:200]
    if p.returncode == 0:
        return "missed", "the scenario passed"
    if p.returncode == 2:
        return "missed", "the scenario reported only unproven results"
    return "missed", f"exit {p.returncode} with {len(fails)} FAIL lines"


def main():
    real = os.environ.get("BIN")
    if not real or not (Path(real) / "sweep").is_file():
        print("BIN must name the folder with release binaries (cargo build --release)")
        return 2
    real = Path(real).resolve()
    cases, errors = load_cases()
    for e in errors:
        print(f"FAIL cases.json: {e}")
    if errors:
        return 2
    want = sys.argv[1] if len(sys.argv) > 1 else ""
    missed = 0
    for case in cases:
        if want and want not in case["scenario"]:
            continue
        with tempfile.TemporaryDirectory(prefix="etudes-mustfire-") as scratch:
            verdict, why = run_case(case, real, scratch)
        missed += verdict != "caught"
        print(f"{verdict:7} {case['scenario']}  [{case['mutant']}] {case['bug']}: {why}", flush=True)
    print(f"{'ok' if not missed else 'FAIL'} mustfire: {missed} planted bug(s) missed")
    return 1 if missed else 0


if __name__ == "__main__":
    sys.exit(main())
