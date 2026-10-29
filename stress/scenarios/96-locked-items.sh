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
if grep -q '1 locked item left alone by flag' "$W/sweep-human.txt"; then
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
DIGEST=$(json_get "$W/export.json" 'd["details"]["plan_digest"]')
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
DIGEST2=$(json_get "$W/export2.json" 'd["details"]["plan_digest"]')
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

# --- unpack: extracts beside a locked file without touching it -----------
python3 -c 'import stat,sys,zipfile; info=zipfile.ZipInfo("note.txt"); info.create_system=3; info.external_attr=(stat.S_IFREG | 0o600) << 16; z=zipfile.ZipFile(sys.argv[1], "w"); z.writestr(info, b"note"); z.close()' "$W/notes.zip"
U="$W/Unpack"
mkdir -p "$U"
printf 'keep' >"$U/locked.txt"
chflags uchg "$U/locked.txt"
assert_exit 0 "unpack extracts next to a locked file" -- "$UNPACK" "$W/notes.zip" --into "$U/out"
[ "$(cat "$U/out/note.txt")" = "note" ] && pass "extracted bytes are exact" || fail "extracted bytes differ"
[ "$(cat "$U/locked.txt")" = "keep" ] && pass "the locked neighbour is untouched" || fail "the locked neighbour changed"

