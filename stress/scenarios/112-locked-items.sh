#!/usr/bin/env bash
# The immutable flag (chflags uchg). A locked file cannot be renamed and a
# locked folder cannot have entries moved into or out of it, so:
#   - sweep holds a locked file by flag, counts it under locked_skipped
#     without its name, and apply/undo move only the others;
#   - an item locked after the plan is exported changes its ctime, so the
#     bound plan is refused before anything moves;
#   - a destination folder locked after planning refuses before any move;
#   - stash leaves the locked item, counts it, and says the folder is NOT
#     clear; pop restores what moved;
#   - unpack extracts beside a locked file without touching it.
# Nothing here is real data. Every flag is cleared on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null

require python3 "reads --json results" || exit 0

json_get() {
  python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(eval(sys.argv[2]))' "$1" "$2"
}

W=$(workdir)
cleanup() {
  chflags -R nouchg "$W" 2>/dev/null || true
  rm -rf "$W"
}
trap cleanup EXIT
export ETUDE_STATE_DIR="$W/state"
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets;print(secrets.token_hex(32))')"

# --- sweep: a locked file is held, counted, never moved ------------------
D="$W/Downloads"
mkdir -p "$D"
for i in 1 2 3 4; do printf 'doc %s' "$i" >"$D/report-$i.pdf"; done
printf 'locked' >"$D/locked-report.pdf"
chflags uchg "$D/locked-report.pdf"
snapshot_tree "$D" "$W/sweep-before.json"
"$SWEEP" "$D" --since 0 --json >"$W/sweep.json"
assert_eq 1 "$(json_get "$W/sweep.json" 'd["details"]["skipped"]["locked_skipped"]')" "locked file has its own skip count"
assert_eq 4 "$(json_get "$W/sweep.json" 'd["details"]["scanned"]')" "only unlocked files are candidates"
if grep -q 'locked-report' "$W/sweep.json"; then
  fail "the locked file's name appeared in the result"
else
  pass "the locked file is counted without its name"
fi
"$SWEEP" "$D" --since 0 >"$W/sweep-human.txt"
if grep -q '1 item is locked or headed for a locked folder, and stays in place' "$W/sweep-human.txt"; then
  pass "human summary discloses the locked count"
else
  fail "human summary omitted the locked count"
fi
assert_exit 0 "apply organises the unlocked files beside the locked one" -- "$SWEEP" apply "$D" --since 0 --yes
[ -e "$D/locked-report.pdf" ] && pass "the locked file stayed in place" || fail "the locked file moved"
assert_exit 0 "undo restores the moved files" -- "$SWEEP" undo "$D"
snapshot_tree "$D" "$W/sweep-after.json"
assert_snapshot_eq "$W/sweep-before.json" "$W/sweep-after.json" "apply and undo leave the folder as found"

# --- sweep: locked after planning is held at move time -------------------
chflags nouchg "$D/locked-report.pdf"
"$SWEEP" "$D" --since 0 --export-plan "$W/plan.bin" --json >"$W/export.json"
DIGEST=$(json_get "$W/export.json" 'd["details"]["binding"]["plan_digest"]')
chflags uchg "$D/report-2.pdf"
snapshot_tree "$D" "$W/late-before.json"
"$SWEEP" apply --plan "$W/plan.bin" --plan-digest "$DIGEST" --yes --json >"$W/late.json"
rc=$?
assert_eq 2 "$rc" "a plan whose item was locked after export is refused"
assert_eq plan_stale "$(json_get "$W/late.json" 'd["reason"]["code"]')" "refused as plan_stale"
snapshot_tree "$D" "$W/late-after.json"
assert_snapshot_eq "$W/late-before.json" "$W/late-after.json" "nothing moved"
chflags nouchg "$D/report-2.pdf"

# --- sweep: a destination folder locked after planning refuses -----------
mkdir -p "$D/Documents"
"$SWEEP" "$D" --since 0 --export-plan "$W/plan2.bin" --json >"$W/export2.json"
DIGEST2=$(json_get "$W/export2.json" 'd["details"]["binding"]["plan_digest"]')
chflags uchg "$D/Documents"
snapshot_tree "$D" "$W/dest-before.json"
"$SWEEP" apply --plan "$W/plan2.bin" --plan-digest "$DIGEST2" --yes --json >"$W/dest.json"
rc=$?
assert_eq 2 "$rc" "a locked destination folder is a refusal"
assert_eq plan_stale "$(json_get "$W/dest.json" 'd["reason"]["code"]')" "refused as plan_stale"
snapshot_tree "$D" "$W/dest-after.json"
assert_snapshot_eq "$W/dest-before.json" "$W/dest-after.json" "nothing moved before the refusal"
chflags nouchg "$D/Documents"

# --- stash: the locked item stays and the folder is NOT clear ------------
S="$W/Desk"
mkdir -p "$S"
printf 'a' >"$S/a.txt"; printf 'b' >"$S/b.txt"; printf 'c' >"$S/locked.txt"
chflags uchg "$S/locked.txt"
"$STASH" "$S" --json >"$W/stash.json"
rc=$?
assert_eq 3 "$rc" "stash ends incomplete when it had to leave a locked item"
assert_eq False "$(json_get "$W/stash.json" 'd["details"]["clear"]')" "stash reports the folder not clear"
assert_eq 1 "$(json_get "$W/stash.json" 'd["details"]["locked_skipped"]')" "stash counts the locked item"
assert_eq 2 "$(json_get "$W/stash.json" 'd["details"]["moved"]')" "stash moved the two unlocked items"
[ -e "$S/locked.txt" ] && pass "stash left the locked item in place" || fail "stash moved the locked item"
assert_exit 0 "stash pop restores the moved items" -- "$STASH" pop "$S"
[ -e "$S/a.txt" ] && [ -e "$S/b.txt" ] && pass "both items are back" || fail "pop did not restore both items"

# --- stash: a folder holding only a locked item is not called clear -------
O="$W/OnlyLocked"
mkdir -p "$O"
printf 'o' >"$O/only.txt"
chflags uchg "$O/only.txt"
"$STASH" "$O" >"$W/only.txt" 2>&1
rc=$?
assert_eq 3 "$rc" "stash exits 3 when the only item is locked"
if grep -q 'NOT clear' "$W/only.txt" && ! grep -q 'already clear' "$W/only.txt"; then
  pass "stash does not call the folder clear"
else
  fail "stash called a folder with a locked item clear"
fi
"$STASH" "$O" --json >"$W/only.json"
assert_eq refused "$(json_get "$W/only.json" 'd["status"]')" "nothing moved, so the result is refused"
assert_eq 1 "$(json_get "$W/only.json" 'd["details"]["locked_skipped"]')" "the locked item is counted"
# The printed step finds the item, and clearing the flag lets stash finish.
assert_eq "$O/only.txt" "$(find "$O" -flags +uchg)" "the printed find step names the locked item"
chflags nouchg "$O/only.txt"
assert_exit 0 "stash finishes once the flag is cleared" -- "$STASH" "$O"
assert_exit 0 "stash pop restores it" -- "$STASH" pop "$O"

# --- sweep and stash: a locked root is said at scan and at apply ----------
R="$W/LockedRoot"
mkdir -p "$R"
for i in 1 2 3 4; do printf 'doc %s' "$i" >"$R/report-$i.pdf"; done
chflags uchg "$R"
"$SWEEP" "$R" --since 0 >"$W/root.txt" 2>&1
if grep -q 'Not ready to apply' "$W/root.txt" && ! grep -q '^Apply:' "$W/root.txt"; then
  pass "a scan does not offer an apply it knows will refuse"
else
  fail "a scan offered an apply into a locked folder"
fi
"$SWEEP" "$R" --since 0 --json >"$W/root.json"
assert_eq True "$(json_get "$W/root.json" 'd["details"]["root_locked"]')" "JSON says the root is locked"
snapshot_tree "$R" "$W/root-before.json"
"$SWEEP" apply "$R" --since 0 --yes >"$W/root-apply.txt" 2>&1
assert_eq 2 "$?" "sweep apply refuses a locked root"
"$STASH" "$R" >"$W/root-stash.txt" 2>&1
assert_eq 2 "$?" "stash refuses a locked root"
for f in root-apply root-stash; do
  if grep -q 'is locked, so nothing can move into or out of it' "$W/$f.txt" && grep -q 'chflags nouchg on the folder' "$W/$f.txt" && [ "$(grep -c 'Nothing was moved' "$W/$f.txt")" = 1 ]; then
    pass "$f names the locked folder and the step"
  else
    fail "$f did not explain the locked folder"
  fi
done
snapshot_tree "$R" "$W/root-after.json"
assert_snapshot_eq "$W/root-before.json" "$W/root-after.json" "a locked root moves nothing"
chflags nouchg "$R"
# The printed step makes a new plan once the lock is cleared.
STEP=$(sed -n 's/.*Then plan again with: sweep //p' "$W/root-apply.txt")
eval "\"\$SWEEP\" $STEP --since 0" >"$W/root-step.txt" 2>&1
assert_eq 0 "$?" "the printed sweep step runs"
if grep -q '^Apply:' "$W/root-step.txt"; then
  pass "the printed sweep step offers the apply"
else
  fail "the printed sweep step did not plan"
fi
assert_exit 0 "apply works once the flag is cleared" -- "$SWEEP" apply "$R" --since 0 --yes

# --- sweep undo: an item locked after apply gets the move denial's step ---
G=$(find "$R" -name report-1.pdf)
chflags uchg "$G"
"$SWEEP" undo "$R" >"$W/undo.txt" 2>&1
assert_eq 3 "$?" "undo stops at the item locked after apply"
if grep -q '1 item is locked, and a locked item cannot be moved' "$W/undo.txt"; then
  pass "undo says the item is locked and how to clear it"
else
  fail "undo did not explain the lock"
fi
chflags nouchg "$G"
assert_exit 0 "undo finishes once the flag is cleared" -- "$SWEEP" undo "$R"
[ -e "$R/report-1.pdf" ] && pass "the item is back" || fail "the item did not come back"

# --- sweep: a group whose folder is locked is held, and the words are true -
H="$W/LockedDest"
mkdir -p "$H/Documents"
for i in 1 2 3 4; do printf 'doc %s' "$i" >"$H/report-$i.pdf"; done
chflags uchg "$H/Documents"
"$SWEEP" "$H" --since 0 >"$W/hold.txt" 2>&1
assert_eq 1 "$?" "a scan with only a held group has nothing to apply"
if grep -Eq '4 items are locked or headed for a locked folder, and stay in place|5 items are locked or headed for a locked folder, and stay in place' "$W/hold.txt"; then
  pass "the held group is explained"
else
  fail "the held group was not explained"
fi
chflags nouchg "$H/Documents"

for f in "$W"/*.txt "$W"/*.json; do
  if grep -q panicked "$f"; then
    fail "panic in $f"
  fi
  if grep -qi 'immutable' "$f" && [ "${f##*.}" = txt ]; then
    fail "internal word in $f"
  fi
done
pass "no panic and no internal word in any output"

# --- unpack: extracts beside a locked file without touching it -----------
python3 -c 'import stat,sys,zipfile; info=zipfile.ZipInfo("note.txt"); info.create_system=3; info.external_attr=(stat.S_IFREG | 0o600) << 16; z=zipfile.ZipFile(sys.argv[1], "w"); z.writestr(info, b"note"); z.close()' "$W/notes.zip"
U="$W/Unpack"
mkdir -p "$U"
printf 'keep' >"$U/locked.txt"
chflags uchg "$U/locked.txt"
assert_exit 0 "unpack extracts next to a locked file" -- "$UNPACK" "$W/notes.zip" --into "$U/out"
[ "$(cat "$U/out/note.txt")" = "note" ] && pass "extracted bytes are exact" || fail "extracted bytes differ"
[ "$(cat "$U/locked.txt")" = "keep" ] && pass "the locked neighbour is untouched" || fail "the locked neighbour changed"

