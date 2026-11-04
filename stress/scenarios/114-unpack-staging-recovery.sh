#!/usr/bin/env bash
# Real SIGKILL, stopped live writers, conservative candidates and competing retries.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require pgrep "the surviving extractor must be stopped" || exit 0
W=$(workdir); trap 'rm -rf "$W"' EXIT
python3 - "$UNPACK" "$W" >"$W/assertions" 2>"$W/errors" <<'PY'
import fcntl
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time
import zipfile

binary, root = sys.argv[1], Path(sys.argv[2])
parent = root / "parent"
parent.mkdir()
dest = parent / "out"
archive = root / "input.zip"
total = 5000
payload = b"synthetic extraction member\n" * 160
def member(z, name, data):
    info = zipfile.ZipInfo(name)
    info.create_system = 3
    info.external_attr = 0o100644 << 16
    z.writestr(info, data)

with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_STORED) as z:
    for n in range(total):
        member(z, f"member-{n:05}.txt", payload)
small = root / "small.zip"
with zipfile.ZipFile(small, "w") as z:
    member(z, "one.txt", b"one\n")
    member(z, "two.txt", b"two\n")
processes = []
logs = []

def check(condition, message):
    print(("ok" if condition else "FAIL") + "\t" + message, flush=True)

def start(source, label):
    log = (root / (label + ".log")).open("wb")
    logs.append(log)
    p = subprocess.Popen([binary, str(source), "--into", str(dest)],
                         stdout=log, stderr=log, start_new_session=True)
    processes.append(p)
    return p

def run(label, source=small):
    p = start(source, label)
    code = p.wait(timeout=30)
    text = (root / (label + ".log")).read_text()
    check(code == 0, label + ": new extraction succeeds")
    expected = {f"member-{n:05}.txt": payload for n in range(total)} if source == archive else {
        "one.txt": b"one\n", "two.txt": b"two\n"}
    actual = {str(p.relative_to(dest)): p.read_bytes() for p in dest.rglob("*") if p.is_file()}
    check(actual == expected, label + ": published paths and bytes are exact, with no internal lock")
    if dest.exists():
        shutil.rmtree(dest)
    return text

def candidate(n, name="out"):
    return parent / f".{name}.unpack-{n:032x}.partial"

def ready(stage):
    stage.mkdir()
    lock = stage / (".unpack-" + stage.name[-40:-8] + ".lock")
    lock.write_bytes(b"unpack staging lock v1\n")
    return lock

try:
    victim = start(archive, "victim")
    stage = None
    kids = []
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline and victim.poll() is None:
        stages = list(parent.glob(".out.unpack-*.partial"))
        if stages:
            files = list(stages[0].glob("member-*.txt"))
            if 0 < len(files) < total:
                os.kill(victim.pid, signal.SIGSTOP)
                kids = [int(p) for p in subprocess.check_output(
                    ["pgrep", "-P", str(victim.pid)], text=True).split()]
                for kid in kids:
                    os.kill(kid, signal.SIGSTOP)
                stage = stages[0]
                break
        time.sleep(0.005)
    witnessed = stage is not None and not dest.exists() and 0 < len(list(stage.glob("member-*.txt"))) < total
    check(witnessed, "witnessed a live extraction with only part of the archive in staging")
    if not witnessed:
        print((root / "victim.log").read_text(), file=sys.stderr)
        raise RuntimeError("could not witness mid-extraction; remaining claims untested")

    legacy = candidate(1); legacy.mkdir(); (legacy / "keep").write_bytes(b"legacy")
    outside = root / "outside"; outside.mkdir(); (outside / "keep").write_bytes(b"outside")
    link = candidate(2); link.symlink_to(outside, target_is_directory=True)
    similar = parent / ".out.unpack-looks-similar.partial"; similar.write_bytes(b"similar")
    ordinary = candidate(3); ordinary.write_bytes(b"ordinary file")
    bad = candidate(4); bad_lock = ready(bad); bad_lock.write_bytes(b"not an initialized lock")
    lock_link = candidate(5); lock_link.mkdir()
    (lock_link / f".unpack-{5:032x}.lock").symlink_to(outside / "keep")
    fifo = candidate(6); fifo.mkdir(); os.mkfifo(fifo / f".unpack-{6:032x}.lock")
    blocked = candidate(7); ready(blocked); blocked.chmod(0)
    other = candidate(8, "another-destination"); ready(other)
    failed_cleanup = candidate(10); ready(failed_cleanup); failed_cleanup.chmod(0o500)
    text = run("live")
    check(stage.is_dir() and str(stage) in text and "left staging candidate" in text,
          "live run's staging remains and is reported by path")
    for path, label in [(legacy, "legacy folder without a lock"), (link, "matching symlink"),
                        (similar, "similar file"), (ordinary, "matching ordinary file"),
                        (bad, "uninitialized lock"), (lock_link, "symlink lock"),
                        (fifo, "FIFO lock"), (blocked, "unreadable lock"),
                        (failed_cleanup, "failed recursive cleanup")]:
        check(os.path.lexists(path) and str(path) in text and
              f"removed abandoned staging {path}" not in text, label + " remains and is reported by path")
    check((legacy / "keep").read_bytes() == b"legacy" and (outside / "keep").read_bytes() == b"outside"
          and similar.read_bytes() == b"similar" and ordinary.read_bytes() == b"ordinary file",
          "retained candidates and symlink target keep their exact bytes")
    check(other.is_dir() and str(other) not in text, "another destination's staging is untouched")

    os.kill(victim.pid, signal.SIGKILL)
    check(victim.wait(timeout=10) == -signal.SIGKILL, "real SIGKILL ended unpack")
    check(stage.is_dir() and not dest.exists(), "SIGKILL leaves staging until a later run")
    text = run("orphan")
    check(stage.is_dir() and str(stage) in text and f"removed abandoned staging {stage}" not in text,
          "a surviving stopped extractor retains its inherited lock and staging")
    for kid in kids:
        os.kill(kid, signal.SIGKILL)
    # Wait for the actual descriptor release, rather than assume kill did it.
    lock_files = list(stage.glob(".unpack-*.lock"))
    if lock_files:
        deadline = time.monotonic() + 10
        with lock_files[0].open("rb") as f:
            while True:
                try:
                    fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    break
                except BlockingIOError:
                    if time.monotonic() >= deadline:
                        raise RuntimeError("extractor did not release lock after SIGKILL")
                    time.sleep(0.01)
    text = run("recovered", archive)
    check(not stage.exists() and f"removed abandoned staging {stage}" in text,
          "retry removes the killed extraction's abandoned staging and reports removal")

    dead = candidate(9); ready(dead)
    first, second = start(small, "race-a"), start(small, "race-b")
    codes = [first.wait(timeout=30), second.wait(timeout=30)]
    check(sorted(codes) == [0, 2], "two simultaneous retries publish once and refuse the existing destination once")
    check(not dead.exists(), "competing recovery runs remove abandoned staging")
    check({p.name: p.read_bytes() for p in dest.iterdir()} == {"one.txt": b"one\n", "two.txt": b"two\n"},
          "competing retries preserve the complete published extraction")
finally:
    for p in processes:
        try:
            os.killpg(p.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        p.wait(timeout=10)
    for log in logs:
        log.close()
    if 'blocked' in locals():
        blocked.chmod(0o700)
    if 'failed_cleanup' in locals():
        failed_cleanup.chmod(0o700)
PY
code=$?
while IFS=$'\t' read -r outcome message; do
  case "$outcome" in ok) pass "$message" ;; FAIL) fail "$message" ;; esac
done <"$W/assertions"
if [ "$code" -ne 0 ]; then
  cat "$W/errors"
  fail "staging recovery witness did not finish (exit $code)"
fi
