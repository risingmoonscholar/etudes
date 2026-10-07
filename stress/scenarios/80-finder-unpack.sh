#!/usr/bin/env bash
# A compact, visible unpack fixture for the recorded Finder demonstration.
#
# The scenario deliberately has the same shape as the live fixture: a ZIP
# holding two directories and three non-empty files.  Its list assertion
# captures stdout and the exit code from ONE `unpack --list` invocation, then
# compares the captured bytes with the complete expected listing.  Checking
# names with separate greps would let a failed listing (or extra members) pass.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir)
trap 'rm -rf "$W"' EXIT

SOURCE="$W/source"
ARCHIVE="$W/demo.zip"
EXTRACTED="$W/extracted"
mkdir -p "$SOURCE/docs" "$SOURCE/images"
printf 'agenda for the Finder unpack demonstration\n' > "$SOURCE/docs/agenda.txt"
printf 'photo bytes for the Finder unpack demonstration\n' > "$SOURCE/images/photo.jpg"
printf 'receipt bytes for the Finder unpack demonstration\n' > "$SOURCE/receipt.pdf"

(cd "$SOURCE" && zip -qr "$ARCHIVE" docs images receipt.pdf)

# Keep the stdout and status from this exact invocation together.  stderr is
# separate on purpose: merging it would make a diagnostic look like a member.
LIST_STDOUT="$W/list.stdout"
LIST_STDERR="$W/list.stderr"
LIST_EXIT=0
"$UNPACK" "$ARCHIVE" --list >"$LIST_STDOUT" 2>"$LIST_STDERR" || LIST_EXIT=$?
assert_eq 0 "$LIST_EXIT" "Finder fixture: one unpack --list invocation succeeds"

EXPECTED_LIST="$W/expected-list.stdout"
printf '%s\n' \
  '' \
  '5 entries in demo' \
  '  docs' \
  '  docs/agenda.txt' \
  '  images' \
  '  images/photo.jpg' \
  '  receipt.pdf' > "$EXPECTED_LIST"
if cmp -s "$EXPECTED_LIST" "$LIST_STDOUT"; then
  pass "Finder fixture: the successful listing is exactly the five planned members"
else
  fail "Finder fixture: unpack --list stdout differed from the complete expected plan"
fi

EXTRACT_EXIT=0
"$UNPACK" "$ARCHIVE" --into "$EXTRACTED" >/dev/null 2>&1 || EXTRACT_EXIT=$?
assert_eq 0 "$EXTRACT_EXIT" "Finder fixture: unpack extracts the planned archive"

EXPECTED_TREE="$W/expected-tree"
ACTUAL_TREE="$W/actual-tree"
printf '%s\n' \
  'docs' \
  'docs/agenda.txt' \
  'images' \
  'images/photo.jpg' \
  'receipt.pdf' > "$EXPECTED_TREE"
(cd "$EXTRACTED" && find . -mindepth 1 -print | sed 's#^./##' | LC_ALL=C sort) > "$ACTUAL_TREE"
if cmp -s "$EXPECTED_TREE" "$ACTUAL_TREE"; then
  pass "Finder fixture: extraction has exactly the planned directories and files"
else
  fail "Finder fixture: extraction paths differed from the complete planned tree"
fi

BYTES_OK=1
for path in docs/agenda.txt images/photo.jpg receipt.pdf; do
  cmp -s "$SOURCE/$path" "$EXTRACTED/$path" || BYTES_OK=0
done
if [ "$BYTES_OK" = 1 ]; then
  pass "Finder fixture: every extracted file has the source bytes"
else
  fail "Finder fixture: an extracted file differed from its archived source bytes"
fi

# A project archive preserves the relative layout that the scanner protects.
PROJECT="$W/project-source"
mkdir -p "$PROJECT/scenes" "$PROJECT/textures"
printf 'document a' > "$PROJECT/scenes/a.blend"
printf 'document b' > "$PROJECT/scenes/b.blend"
printf 'texture' > "$PROJECT/textures/wood.png"
(cd "$PROJECT" && zip -qr "$W/project.zip" scenes textures)
assert_exit 0 "unpack extracts a project with sibling assets" -- "$UNPACK" "$W/project.zip" --into "$W/project-out"
for path in scenes/a.blend scenes/b.blend textures/wood.png; do
  cmp -s "$PROJECT/$path" "$W/project-out/$path" \
    && pass "project unpack preserved $path and its bytes" \
    || fail "project unpack changed $path or its bytes"
done

# Drive real extraction transactions. The observer below checks visibility
# while gzip is writing and injects members only into disposable staging.
# These are audit checks, independent of the archive header preflight.
TRANSACTION_EXIT=0
python3 - "$UNPACK" "$W" <<'PY_TRANSACTIONS' || TRANSACTION_EXIT=$?
import gzip
import os
import pathlib
import re
import stat
import subprocess
import sys
import tarfile
import time
import zipfile

unpack, work = sys.argv[1], pathlib.Path(sys.argv[2])


def require(condition, message):
    if not condition:
        raise AssertionError(message)
    print("      transaction: " + message, flush=True)


def leftovers(dest):
    return list(dest.parent.glob("." + dest.name + ".unpack-*.partial"))


# All formats expand to identical 16 MiB payloads. Two budgets and three
# repetitions exercise fast completion and crossing the polling threshold.
size = 16 * 1024 * 1024
payload = work / "payload.bin"
payload.write_bytes(b"x" * size)
archives = [work / "measure.zip", work / "measure.tar", work / "measure.gz"]
with zipfile.ZipFile(archives[0], "w", zipfile.ZIP_DEFLATED) as archive:
    archive.write(payload, "payload.bin")
with tarfile.open(archives[1], "w") as archive:
    archive.add(payload, arcname="payload.bin")
with gzip.open(archives[2], "wb", compresslevel=1) as archive:
    archive.write(payload.read_bytes())
max_overshoot = 0
for archive in archives:
    for budget in (1, 1024 * 1024):
        for repetition in range(3):
            dest = work / "measured-out"
            result = subprocess.run(
                [unpack, str(archive), "--into", str(dest), "--max-size", str(budget)],
                capture_output=True, text=True, timeout=30,
            )
            require(result.returncode == 2, "soft-limit refusal exits 2")
            match = re.search(r"(\d+) bytes measured after stopping;\s+limit (\d+) bytes; overshoot (\d+) bytes", result.stderr)
            require(match is not None, "soft-limit refusal reports exact final total, budget and overshoot")
            total, reported_budget, overshoot = map(int, match.groups())
            require(budget < total <= size and reported_budget == budget and overshoot == total - budget,
                    "soft-limit measurements agree with the actual payload and budget")
            require(not dest.exists() and not leftovers(dest) and "Nothing was left behind." in result.stderr,
                    "soft-limit failure leaves no destination or staging and reports checked cleanup")
            max_overshoot = max(max_overshoot, overshoot)
print(f"      MEASURED_MAX_OVERSHOOT_BYTES={max_overshoot}", flush=True)

# A long enough stream to observe the extraction boundary without adding a
# hook to the shipped binary. Stop the wrapper briefly after finding staging
# so the observer can install a deterministic audit fixture before it resumes.
large = work / "transaction.gz"
large_size = 128 * 1024 * 1024
with gzip.open(large, "wb", compresslevel=1) as archive:
    for _ in range(128):
        archive.write(b"x" * (1024 * 1024))


def transaction(tag, injected=None):
    import signal
    dest = work / tag
    process = subprocess.Popen(
        [unpack, str(large), "--into", str(dest), "--max-size", "1G", "--json"],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    deadline = time.monotonic() + 30
    observed_staging = False
    injected_path = None
    complete_at_visibility = True
    try:
        while process.poll() is None:
            if time.monotonic() > deadline:
                raise AssertionError("transaction did not finish before deadline")
            staging = leftovers(dest)
            if staging and not observed_staging:
                os.kill(process.pid, signal.SIGSTOP)
                try:
                    stage = staging[0]
                    require(stage.parent == dest.parent, "staging is a sibling on the destination volume")
                    require(stat.S_IMODE(stage.stat().st_mode) == 0o700, "staging is private (mode 0700)")
                    require(not dest.exists(), "destination is absent during extraction into staging")
                    if injected == "symlink":
                        injected_path = stage / "unexpected-link.txt"
                        injected_path.symlink_to(work / "outside-sentinel")
                    elif injected == "fifo":
                        injected_path = stage / "unexpected-fifo.txt"
                        os.mkfifo(injected_path)
                    observed_staging = True
                finally:
                    os.kill(process.pid, signal.SIGCONT)
            if dest.exists():
                complete_at_visibility &= (dest / "transaction").stat().st_size == large_size
            time.sleep(0.0005)
        stdout, stderr = process.communicate(timeout=5)
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate()
    require(observed_staging, "observer actually reached private staging before publication")
    if injected:
        require(process.returncode == 2, "post-extraction audit refuses the injected " + injected)
        kind = "symlink" if injected == "symlink" else "special member"
        expected = (f"unpack: staging audit failed ({kind} at {injected_path.parent}/<name.txt>); "
                    "destination was not published. Nothing was left behind.\n")
        require(stderr == expected,
                "audit diagnostic gives the exact staging path, member type, redacted name and cleanup result: " + repr(stderr))
        require(not dest.exists() and not leftovers(dest) and "Nothing was left behind." in stderr,
                "failed audit leaves no destination or staging and reports exactly what remains (nothing)")
    else:
        import json
        result = json.loads(stdout)
        require(process.returncode == 0 and result["paths_audited"] == 1,
                "successful extraction reports its final tree audit")
        require(complete_at_visibility and (dest / "transaction").read_bytes() == b"x" * large_size,
                "first visible destination contains the complete byte-identical payload")
        require(not leftovers(dest), "atomic publication consumes staging")


(work / "outside-sentinel").write_bytes(b"outside staging must stay untouched")
transaction("atomic-out")
transaction("audit-link-out", "symlink")
transaction("audit-fifo-out", "fifo")
require((work / "outside-sentinel").read_bytes() == b"outside staging must stay untouched",
        "containment audit and cleanup never follow an external link")
PY_TRANSACTIONS
assert_eq 0 "$TRANSACTION_EXIT" "staged extraction, containment audit, atomic publication and failure cleanup"

exit 0
