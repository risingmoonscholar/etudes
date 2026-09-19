#!/usr/bin/env bash
# Compare every direct scenario with the batch runner, including the original
# visible assertion counts and the unchanged runner from origin/main. Real
# executions may expose a timing-dependent difference; report it rather than silently accepting a changed count.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
cargo build --release --quiet || exit 1
export BIN="$PWD/target/release"
exec python3 - "$@" <<'PYTHON'
import collections
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

root = Path.cwd()
env = dict(os.environ)
for key in ("STRESS_WRAP_DEPTH", "STRESS_WRAPPER_PID", "STRESS_RESULT_FD", "STRESS_STATUS_FD"):
    env.pop(key, None)
failed = []

def counts(output):
    return tuple(len(re.findall(r"^    " + tag + r" ", output, re.M))
                 for tag in ("ok", "FAIL", "unproven"))

def outcome(c):
    return 1 if c[1] else (0 if c[0] else 2)

with tempfile.TemporaryDirectory(prefix="etudes-outcome-check-") as tmp:
    direct = {}
    for scenario in sorted((root / "stress/scenarios").glob("*.sh")):
        record = Path(tmp) / "record"
        run_env = dict(env, SCENARIO=scenario.stem, STRESS_RESULT_FD="197")
        result = subprocess.run(
            ["/bin/bash", "-c", 'exec 197>"$1"; exec /bin/bash "$2"',
             "--", str(record), str(scenario)], env=run_env,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        data = collections.Counter(record.read_text().splitlines())
        c = tuple(data[tag] for tag in ("ok", "FAIL", "unproven"))
        direct[scenario.stem] = (c, result.returncode)
        if c != counts(result.stdout) or outcome(c) != result.returncode:
            failed.append(f"{scenario.stem}: direct record {c}, visible {counts(result.stdout)}, exit {result.returncode}")
        print(f"direct {scenario.stem}: {c}, exit {result.returncode}", flush=True)
    # Only the runner differs in this view: scenarios, lib.sh, source and
    # release binaries are the exact same files used by the candidate run.
    baseline_root = Path(tmp) / "baseline"
    baseline_root.mkdir()
    for entry in root.iterdir():
        if entry.name not in ("stress", ".git"):
            (baseline_root / entry.name).symlink_to(entry, target_is_directory=entry.is_dir())
    (baseline_root / "stress").mkdir()
    for entry in (root / "stress").iterdir():
        if entry.name != "run.sh":
            (baseline_root / "stress" / entry.name).symlink_to(entry, target_is_directory=entry.is_dir())
    baseline_ref = subprocess.check_output(
        ["git", "rev-parse", "origin/main"], text=True).strip()
    baseline_runner = subprocess.check_output(
        ["git", "show", f"{baseline_ref}:stress/run.sh"])
    (baseline_root / "stress/run.sh").write_bytes(baseline_runner)
    print(f"baseline runner: origin/main at {baseline_ref}; identical candidate scenarios", flush=True)
    baseline = subprocess.run(["/bin/bash", "stress/run.sh"], cwd=baseline_root,
                              env=env, stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT, text=True)
    print(baseline.stdout, end="", flush=True)
    baseline_sections = re.split(r"^── ([^\n]+)\n", baseline.stdout, flags=re.M)
    baseline_counts = {name: counts(output) for name, output in
                       zip(baseline_sections[1::2], baseline_sections[2::2])}
    if set(baseline_counts) != set(direct):
        failed.append("origin/main runner did not execute exactly the complete scenario set")
    baseline_totals = tuple(sum(c[i] for c in baseline_counts.values()) for i in range(3))
    baseline_summary = re.search(r"  passed +([0-9]+)\n  failed +([0-9]+)\n  unproven +([0-9]+)", baseline.stdout)
    if not baseline_summary or tuple(map(int, baseline_summary.groups())) != baseline_totals:
        failed.append("origin/main runner displayed totals disagree with its scenario assertions")
    if baseline.returncode != outcome(baseline_totals):
        failed.append(f"origin/main runner exit {baseline.returncode} disagrees with totals {baseline_totals}")
    # Observe the scenario process independently of run.sh's PIPESTATUS record.
    # The shim preserves every inherited descriptor and delegates to /bin/bash.
    observer_dir = Path(tmp) / "observer-bin"
    observer_dir.mkdir()
    observed_record = Path(tmp) / "observed-statuses"
    observed_record.touch()
    observer = observer_dir / "bash"
    observer.write_text("#!" + sys.executable + "\n" + r'''import os
from pathlib import Path
import subprocess
import sys
args = sys.argv[1:]
status = subprocess.call(["/bin/bash", *args], close_fds=False)
status = 128 - status if status < 0 else status
if len(args) == 1 and args[0].startswith("stress/scenarios/"):
    with open(os.environ["STRESS_OBSERVED_STATUSES"], "a") as record:
        record.write(f"{Path(args[0]).stem}\t{status}\n")
sys.exit(status)
''')
    observer.chmod(0o755)
    batch_env = dict(env, PATH=str(observer_dir) + os.pathsep + env["PATH"],
                     STRESS_OBSERVED_STATUSES=str(observed_record))
    status_record = Path(tmp) / "batch-statuses"
    batch = subprocess.run(
        ["/bin/bash", "-c",
         'exec 196>"$1"; export STRESS_STATUS_FD=196; exec /bin/bash stress/run.sh',
         "--", str(status_record)], env=batch_env,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    batch_statuses = {}
    for line in status_record.read_text().splitlines():
        fields = line.split("\t")
        if len(fields) != 2 or not fields[1].isdigit():
            failed.append(f"invalid batch child status record: {line!r}")
            continue
        name, status = fields
        if name in batch_statuses:
            failed.append(f"duplicate batch child status: {name}")
        batch_statuses[name] = int(status)
    if set(batch_statuses) != set(direct):
        failed.append("runner did not report an actual child status for exactly every scenario")
    observed_statuses = {}
    for line in observed_record.read_text().splitlines():
        name, status = line.split("\t")
        if name in observed_statuses:
            failed.append(f"duplicate independently observed child: {name}")
        observed_statuses[name] = int(status)
    if set(observed_statuses) != set(direct):
        failed.append("independent observer did not wait for exactly every scenario")
    if observed_statuses != batch_statuses:
        failed.append(f"runner child statuses {batch_statuses} disagree with independent waits {observed_statuses}")
    print(batch.stdout, end="", flush=True)
    sections = re.split(r"^── ([^\n]+)\n", batch.stdout, flags=re.M)
    seen = set()
    totals = [0, 0, 0]
    for name, output in zip(sections[1::2], sections[2::2]):
        c = counts(output)
        seen.add(name)
        if baseline_counts.get(name) != c:
            failed.append(f"{name}: origin/main {baseline_counts.get(name)}, candidate runner {c}")
        totals = [a + b for a, b in zip(totals, c)]
        actual_status = batch_statuses.get(name)
        if direct.get(name) != (c, actual_status):
            failed.append(f"{name}: direct {direct.get(name)}, runner {(c, actual_status)}")
        print(f"compare {name}: direct {direct.get(name)}, "
              f"runner {(c, actual_status)}, origin/main {baseline_counts.get(name)}", flush=True)
    if seen != set(direct):
        failed.append("runner did not execute exactly the complete direct scenario set")
    summary = re.search(r"  passed +([0-9]+)\n  failed +([0-9]+)\n  unproven +([0-9]+)", batch.stdout)
    if not summary or tuple(map(int, summary.groups())) != tuple(totals):
        failed.append("runner displayed totals disagree with its scenario assertions")
    if batch.returncode != outcome(totals):
        failed.append(f"runner exit {batch.returncode} disagrees with totals {totals}")
    for message in failed:
        print("FAIL " + message)
    if failed:
        sys.exit(1)
    print(f"ok: all {len(direct)} scenarios agree across direct, candidate runner and origin/main runner on pass/FAIL/unproven counts and exit outcome")
PYTHON
