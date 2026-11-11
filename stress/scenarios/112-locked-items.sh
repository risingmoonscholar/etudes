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

W=$(workdir) || exit 1
W=$(cd "$W" && pwd -P) || exit 1
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
[ -z "$(xattr -p com.apple.metadata:_kMDItemUserTags "$D/locked-report.pdf" 2>/dev/null)" ] \
  && pass "apply leaves the locked file's tags untouched" || fail "apply changed the locked file's tags"
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
"$STASH" "$O" >"$W/only.txt" 2>&1
"$STASH" "$O" --json >"$W/only.json"
assert_eq refused "$(json_get "$W/only.json" 'd["status"]')" "nothing moved, so the result is refused"
assert_eq 1 "$(json_get "$W/only.json" 'd["details"]["locked_skipped"]')" "the locked item is counted"
# The printed step finds the item, and clearing the flag lets stash finish.
STEP=$(sed -n 's/.*See which with: //p' "$W/only.txt" | head -1)
[ -n "$STEP" ] && assert_eq "$O/only.txt" "$(eval "$STEP")" "the printed find step names the locked item" || fail "locked refusal omitted the find step"
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

# A tags apply must hold locked items as well as a move apply.
# Tags are read through libc getxattr (macOS Python has no os.getxattr), as
# 91 does. A missing tag is "none"; any other read error stops the helper,
# and that is a FAIL, never an empty string compared with an empty string.
cat > "$W/tagtool.py" <<'PYTAG'
import ctypes, ctypes.util, errno, json, os, pathlib, plistlib, stat, sys

NAME = b"com.apple.metadata:_kMDItemUserTags"
libc = ctypes.CDLL(ctypes.util.find_library("c"), use_errno=True)
for fn in (libc.getxattr, libc.setxattr):
    fn.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint32, ctypes.c_int]
libc.getxattr.restype = ctypes.c_ssize_t
libc.setxattr.restype = ctypes.c_int
NOFOLLOW = 1
ENOATTR = 93


def get(path):
    size = libc.getxattr(os.fsencode(path), NAME, None, 0, 0, NOFOLLOW)
    if size < 0:
        code = ctypes.get_errno()
        if code == ENOATTR:
            return None
        raise OSError(code, f"getxattr: {os.strerror(code)}")
    buf = ctypes.create_string_buffer(size)
    got = libc.getxattr(os.fsencode(path), NAME, buf, size, 0, NOFOLLOW)
    if got < 0:
        code = ctypes.get_errno()
        raise OSError(code, f"getxattr: {os.strerror(code)}")
    return buf.raw[:got]


if sys.argv[1] == "set":
    data = plistlib.dumps(sys.argv[3:], fmt=plistlib.FMT_BINARY)
    if libc.setxattr(os.fsencode(sys.argv[2]), NAME, data, len(data), 0, NOFOLLOW) != 0:
        raise OSError(ctypes.get_errno(), "setxattr")
elif sys.argv[1] == "locked":
    rows = {}
    for p in sorted(pathlib.Path(sys.argv[2]).rglob('*')):
        if p.is_file() and not p.is_symlink() and p.lstat().st_flags & stat.UF_IMMUTABLE:
            value = get(p)
            rows[str(p)] = None if value is None else value.hex()
    print(json.dumps(rows, sort_keys=True))
else:
    sys.exit(f"unknown command {sys.argv[1]}")
PYTAG
T="$W/tag-locked"; mkdir -p "$T"
for i in 1 2 3 4; do printf 'tag document %s' "$i" > "$T/report-$i.pdf"; done
printf 'locked tag document' > "$T/locked.pdf"; chflags uchg "$T/locked.pdf"
# A locked file that already has a tag, so the before/after compare holds a
# real value: a tag run that rewrites a locked file's tags changes it.
printf 'locked tagged document' > "$T/locked-tagged.pdf"
python3 "$W/tagtool.py" set "$T/locked-tagged.pdf" $'Blue\n4' || fail "could not tag the locked-tagged fixture"
chflags uchg "$T/locked-tagged.pdf"
"$SWEEP" "$T" --scheme tags --map pdf=Filed --since 0 --export-plan "$W/tags2.plan" --json > "$W/tag-export.json"
assert_eq 0 "$?" "export a tags plan"
TAG_DIGEST=$(json_get "$W/tag-export.json" 'd["details"]["binding"]["plan_digest"]')
if LOCKED_TAGS_BEFORE=$(python3 "$W/tagtool.py" locked "$W" 2>"$W/tagtool.err"); then
  TAGGED_BEFORE=$(printf '%s' "$LOCKED_TAGS_BEFORE" | python3 -c 'import json,sys; print(sum(v is not None for v in json.load(sys.stdin).values()))')
  [ "$TAGGED_BEFORE" -ge 1 ] && pass "$TAGGED_BEFORE locked fixture file(s) carry a tag before the tag apply" \
    || fail "no locked fixture file carries a tag, so the compare would prove nothing"
else
  fail "could not read the locked files' tags before the tag apply: $(cat "$W/tagtool.err")"
fi
assert_exit 0 "tag apply processes unlocked files" -- "$SWEEP" apply --plan "$W/tags2.plan" --plan-digest "$TAG_DIGEST" --yes
if LOCKED_TAGS_AFTER=$(python3 "$W/tagtool.py" locked "$W" 2>"$W/tagtool.err"); then
  assert_eq "$LOCKED_TAGS_BEFORE" "$LOCKED_TAGS_AFTER" "tag apply preserves the tags of every locked fixture file"
else
  fail "could not read the locked files' tags after the tag apply: $(cat "$W/tagtool.err")"
fi
assert_eq '{}' "$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(json.dumps({k: v for k, v in d.items() if k.endswith("/tag-locked/locked.pdf") and v is not None}))' "${LOCKED_TAGS_AFTER:-null}" 2>&1)" "tag apply leaves the locked file untagged"

# Lock a pending item after planning, while the apply is held by its watcher.
# The watcher stops apply after the first journalled move it sees, and apply
# can move more items before the stop lands, so it cannot promise to stop just
# before a chosen item. The victim is therefore the SECOND-TO-LAST item in the
# order apply moves (the plan's member order, which the journal keeps): a
# confirmed stop before the last two moves leaves it pending, and the last
# member still has to move after the hold, so an apply that stops at the held
# item fails. The watcher's own index and total must agree with that plan, and
# the process must be in state T. A trial whose
# apply ended first is checked as a whole apply, then retried on a fresh folder;
# only four missed windows in a row are unproven.
DUMP="$BIN/journal-dump"
LATE_N=300
LOCKED=0
if [ -x "$DUMP" ]; then
  for trial in 1 2 3 4; do
    L="$W/late-lock-$trial"; mkdir -p "$L"
    for i in $(seq 1 "$LATE_N"); do printf 'late payload %s' "$i" > "$L/report-$i.pdf"; done
    PICK=$("$SWEEP" "$L" --since 0 --json 2>/dev/null | python3 -c '
import json, sys
groups = json.load(sys.stdin)["details"]["groups"]
members = [m for g in groups for m in g["members"]]
print(len(members) - 2)
print(members[-2])
print(members[-1])
')
    VICTIM_INDEX=$(sed -n 1p <<< "$PICK")
    VICTIM=$(sed -n 2p <<< "$PICK")
    LAST=$(sed -n 3p <<< "$PICK")
    if [ "$VICTIM_INDEX" != "$((LATE_N - 2))" ] || [ ! -f "$VICTIM" ] || [ ! -f "$LAST" ]; then
      fail "the late-lock plan does not list all $LATE_N files: $PICK"
      break
    fi
    "$SWEEP" apply "$L" --since 0 --yes >"$W/late-lock.out" 2>&1 & PID=$!
    if WITNESS=$("$DUMP" --watch-apply "$ETUDE_STATE_DIR" "$L" "$VICTIM" "$PID" 2>"$W/lock-watch.err"); then
      read -r WATCH_MOVED WATCH_TOTAL WATCH_INDEX <<< "$WITNESS"
      if [ "$WATCH_TOTAL" = "$LATE_N" ] && [ "$WATCH_INDEX" = "$VICTIM_INDEX" ] \
          && ps -o state= -p "$PID" | grep -q T && [ -f "$VICTIM" ]; then
        chflags uchg "$VICTIM"
        LOCKED=1
        break
      fi
      echo "    note     trial $trial: watcher stop did not match the plan ($WITNESS of $LATE_N, victim $VICTIM_INDEX)"
    else
      echo "    note     trial $trial: $(cat "$W/lock-watch.err")"
    fi
    # A missed window: the apply ran on its own. It must still be a whole,
    # correct apply, so a broken run is a FAIL here, never a retry. A good
    # run adds no pass, so the pass count is the same whichever trial locks.
    kill -CONT "$PID" 2>/dev/null || true; wait "$PID"; RC=$?
    LEFT=$(find "$L" -maxdepth 1 -type f | wc -l | tr -d ' ')
    if [ "$RC" != 0 ] || [ "$LEFT" != 0 ]; then
      fail "trial $trial apply that ran past the watcher exited $RC and left $LEFT of $LATE_N files: $(cat "$W/late-lock.out")"
      break
    fi
    chflags -R nouchg "$L" 2>/dev/null; rm -rf "$L"
  done
  if [ "$LOCKED" = 1 ]; then
    pass "pending item locked while apply was confirmed stopped ($WATCH_MOVED/$WATCH_TOTAL moved, victim at $WATCH_INDEX, trial $trial)"
    kill -CONT "$PID"; wait "$PID"; RC=$?
    assert_eq 0 "$RC" "late lock is held while the other moves finish"
    assert_eq "$VICTIM" "$(find "$L" -maxdepth 1 -type f)" "only the late-locked item is left in the source folder"
    LAST_NAME=$(basename "$LAST")
    [ ! -e "$LAST" ] && [ "$(find "$L" -mindepth 2 -type f -name "$LAST_NAME" | wc -l | tr -d ' ')" = 1 ] \
      && pass "the last planned item moved after the hold" || fail "the last planned item did not move after the hold"
    grep -qx "Moved $((LATE_N - 1)) files." "$W/late-lock.out" \
      && pass "apply reports $((LATE_N - 1)) moved" || fail "apply did not report $((LATE_N - 1)) moved: $(cat "$W/late-lock.out")"
    grep -q '^1 item was locked after the scan and stays in place\.' "$W/late-lock.out" \
      && pass "apply counts and reports the late-locked item" || fail "apply did not report the late-locked item: $(cat "$W/late-lock.out")"
    VICTIM_NAME=$(basename "$VICTIM" .pdf)
    assert_eq "late payload ${VICTIM_NAME#report-}" "$(cat "$VICTIM" 2>/dev/null)" "late-locked item stays with its bytes"
    chflags nouchg "$VICTIM"
    STEP=$(sed -n 's/^Undo with: sweep //p' "$W/late-lock.out" | head -1)
    if [ -n "$STEP" ]; then
      eval "\"\$SWEEP\" $STEP" >"$W/late-recovery.out" 2>&1
      assert_eq 0 "$?" "printed late-lock recovery step succeeds"
    else
      fail "partial apply omitted its printed recovery step: $(cat "$W/late-lock.out")"
    fi
    assert_eq "$LATE_N" "$(find "$L" -maxdepth 1 -type f | wc -l | tr -d ' ')" "late-lock recovery returns every file"
  elif [ "$FAILED" -eq 0 ]; then
    unproven "late lock during apply" "no confirmed stopped apply with the second-to-last planned item pending across four trials: $(cat "$W/lock-watch.err")"
  fi
else
  unproven "late lock during apply" "journal watcher unavailable"
fi
