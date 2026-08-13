#!/usr/bin/env bash
# Deepest finding in this family, and now a regression guard rather than a
# reproduction: `undo()` used to lack the crash-safe incremental journaling
# that `apply()` has. apply() calls record_done() -- appended, sealed,
# fsync'd -- after every successful move, so a crash at any point leaves a
# journal that describes exactly what happened. undo()'s loop had no
# equivalent: it mutated state in memory only, and the one place that
# persisted it was reached ONLY on the Ok path. If undo errored partway
# through, every file it had already physically restored was silently dropped
# from both the report and the on-disk journal.
#
# Fixed for issue #7: undo calls record_undone() per reversal, the mirror of
# record_done. This scenario stays because the hazard it builds is real and
# the guarantee is worth re-checking, not because the defect is still there.
#
# This is demonstrated with a real, deterministic volume hazard rather than
# disk-fill timing luck: a shared-token group whose members live on two
# volumes, one of which is remounted read-only between apply and undo. Undo
# processes entries in reverse order, so ordering the group correctly (by
# path, which is scan's sort key) guarantees the writable-origin entries are
# restored FIRST and the now-read-only-origin entries fail AFTER -- a
# genuine, repeatable partial-undo-then-error, no byte-level luck required.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require hdiutil "this scenario needs a real disk image to remount read-only" || exit 0

IMG=""
INNER=""
cleanup() {
  [ -n "$INNER" ] && [ -d "$INNER" ] && hdiutil detach "$INNER" -force >/dev/null 2>&1
  [ -n "${W:-}" ] && rm -rf "$W"
}
trap cleanup EXIT

W=$(workdir)
OUTER="$W/outer"
IMG="$W/inner.dmg"
# Sorts before the group's own filenames, so its entries get LOWER indices
# in the journal's entry list -- and therefore get processed LAST by undo's
# reverse-order loop, i.e. after the writable-origin entries have already
# been restored.
INNER="$OUTER/aaa_ro_source"
mkdir -p "$OUTER" "$INNER"

if ! hdiutil create -size 20m -fs "APFS" -volname UndoProgressStress "$IMG" >/dev/null 2>&1; then
  unproven "undo does not silently drop partial-restore progress on error" "hdiutil create failed on this host"
  exit 0
fi
if ! hdiutil attach "$IMG" -mountpoint "$INNER" -nobrowse >/dev/null 2>&1; then
  unproven "undo does not silently drop partial-restore progress on error" "hdiutil attach failed on this host"
  exit 0
fi

for f in delta echo foxtrot; do : > "$OUTER/stuck_$f.txt"; done
for f in alpha bravo; do : > "$INNER/stuck_$f.txt"; done

assert_exit 0 "setup: apply succeeds while both halves are writable" -- "$SWEEP" apply "$OUTER" --yes --depth 2
if [ ! -f "$OUTER/stuck/stuck_alpha.txt" ] || [ ! -f "$OUTER/stuck/stuck_delta.txt" ]; then
  fail "setup: apply did not produce the expected group layout -- cannot continue this scenario"
  exit 0
fi

# Flip the inner volume read-only, then undo. The three outer-origin entries
# must restore fine; the two inner-origin ones must fail with EROFS.
hdiutil detach "$INNER" -force >/dev/null 2>&1
if ! hdiutil attach "$IMG" -mountpoint "$INNER" -nobrowse -readonly >/dev/null 2>&1; then
  unproven "undo does not silently drop partial-restore progress on error" "could not reattach read-only on this host"
  exit 0
fi

UNDO1_OUT=$("$SWEEP" undo 2>&1)
UNDO1_CODE=$?

RESTORED_TO_OUTER=0
for f in delta echo foxtrot; do [ -f "$OUTER/stuck_$f.txt" ] && RESTORED_TO_OUTER=$((RESTORED_TO_OUTER+1)); done
STILL_STUCK=0
for f in alpha bravo; do [ -f "$OUTER/stuck/stuck_$f.txt" ] && STILL_STUCK=$((STILL_STUCK+1)); done

if [ "$RESTORED_TO_OUTER" != 3 ] || [ "$STILL_STUCK" != 2 ]; then
  # The engineered split didn't land the way it was designed to -- report
  # honestly rather than asserting on a premise that didn't hold this run.
  unproven "undo does not silently drop partial-restore progress on error" \
    "expected a 3-restored/2-stuck split after the read-only remount, got restored=$RESTORED_TO_OUTER stuck=$STILL_STUCK (undo exit=$UNDO1_CODE output=[$UNDO1_OUT])"
  exit 0
fi

pass "reproduced a genuine partial undo: 3 files physically restored, then undo hit a read-only volume and stopped"

if [ "$UNDO1_CODE" = 0 ]; then
  fail "undo reported success (exit 0) while 2 of 5 files are still stuck in the holding directory on a read-only volume"
else
  pass "undo did not report success while files remain unrestored"
fi

# THE DEFECT: three files were just, in physical reality, moved back to
# $OUTER. cmd_undo's Err branch never reports UndoReport.restored -- it only
# prints the raw io error. A user watching this would have zero indication
# that anything happened at all.
if echo "$UNDO1_OUT" | grep -qi "restored"; then
  pass "undo's error-path output does mention the files it restored before failing (defect not present / already fixed)"
else
  fail "REAL DEFECT: sweep undo silently restored 3 of 5 files (confirmed on disk) but its \
output on the failing call was: [$UNDO1_OUT] -- no 'Restored N files' anywhere. A user who \
stops here (a completely reasonable reaction to an unqualified io error) has no way to know \
undo made any progress at all. Root cause: etude-core/src/apply.rs's undo() uses \
move_one(...).map_err(ApplyError::Io)? inside its loop, which discards the accumulated \
UndoReport (and its .restored count) the instant one entry fails -- unlike apply()'s loop, \
which persists progress via record_done() after every single successful move specifically so \
a partial run is never silently lost. sweep-cli's cmd_undo compounds this: its Err arm never \
calls j.save_sealed(&sl), so the on-disk journal is not updated either -- it continues to claim \
all 5 entries are still 'done' (i.e. still in the holding dir) when 3 of them, in physical \
reality, are not."
fi

# Confirm the journal desync directly: does the on-disk journal still claim
# the 3 already-restored entries as done? Detected via the SAME symptom the
# tool itself reports on a second undo call: it explains them away as
# "already gone" rather than recognising they were already restored.
hdiutil detach "$INNER" -force >/dev/null 2>&1
hdiutil attach "$IMG" -mountpoint "$INNER" -nobrowse >/dev/null 2>&1

UNDO2_OUT=$("$SWEEP" undo 2>&1)
UNDO2_CODE=$?
assert_eq 0 "$UNDO2_CODE" "a second undo call (volume writable again) finishes the job"

FULLY_RESTORED=1
for f in "$OUTER/stuck_delta.txt" "$OUTER/stuck_echo.txt" "$OUTER/stuck_foxtrot.txt" \
         "$INNER/stuck_alpha.txt" "$INNER/stuck_bravo.txt"; do
  [ -f "$f" ] || FULLY_RESTORED=0
done
if [ "$FULLY_RESTORED" = 1 ]; then
  pass "eventually (after a second call) all 5 files did end up safely restored -- no data was permanently lost"
else
  fail "REAL DEFECT (worse than the above): after two undo calls, not every file made it back. Second call output: [$UNDO2_OUT]"
fi

if echo "$UNDO2_OUT" | grep -qi "already gone"; then
  fail "the journal-desync side effect: the second call describes the 3 files that were ALREADY \
successfully restored by the first (failed) call as '...were already gone' -- conflating \
'silently restored earlier, unreported' with 'the destination file is missing'. Output: [$UNDO2_OUT]"
else
  pass "the second call's report does not misdescribe the earlier silent restores as missing files"
fi

exit 0
