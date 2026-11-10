#!/usr/bin/env bash
# Two applies of one folder are two operations, and each needs its own undo.
# After the first undo the folder is not yet as it was, so undo must say that
# another apply of that folder can still be undone and give a step that works.
# After the last one it says nothing more, and the folder matches the original.
#
# The second apply is made by adding new files after the first one finished,
# and `sweep status` confirms both are on record and reversible before any undo
# runs. Nothing waits and nothing depends on stopping a run part way.
#
# The folder names hold a space, a single quote and a tab, so the printed step
# is quoted both ways sweep quotes a path, and each step is run exactly as
# printed. A name with a byte that is not UTF-8 is tried too; APFS refuses such
# a name, and then that case is unproven, not passed.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir) || exit 1; trap 'rm -rf "${W:?}"' EXIT
W=$(cd "${W:?}" && pwd -P) && [ -n "$W" ] || exit 1
require python3 "these checks read JSON with Python" || exit 0
export HOME="$W/home"; mkdir -p "$HOME"

has() { case "$2" in *"$3"*) pass "$1" ;; *) fail "$1: wanted '$3' in: $2" ;; esac; }
hasnt() { case "$2" in *"$3"*) fail "$1: found '$3' in: $2" ;; *) pass "$1" ;; esac; }
field() {  # field 'details.remaining_reversible' FILE
  python3 -I - "$1" "$2" <<'PY'
import json, sys
value = json.load(open(sys.argv[2]))
for key in sys.argv[1].split("."):
    value = value[key]
print(value)
PY
}
reversible() {  # reversible DIR: how many applies of DIR status says can be undone
  "$SWEEP" status "$1" --json >"$W/status.json" 2>/dev/null </dev/null
  python3 -I - "$W/status.json" <<'PY'
import json, sys
details = json.load(open(sys.argv[1]))["details"]
rows = details if isinstance(details, list) else [details]
print(sum(1 for row in rows if row.get("command")))
PY
}
# fixture DIR: two kinds of files in DIR, and a third kind held aside in
# DIR.later. The original is all three kinds in DIR.
fixture() {
  local dir="$1" i
  mkdir -p "$dir" "$dir.later"
  for i in 1 2 3 4; do
    printf 'document %s\n' "$i" >"$dir/document_$i.pdf"
    printf 'photo %s\n' "$i" >"$dir/photo_$i.jpg"
    printf 'archive %s\n' "$i" >"$dir/archive_$i.zip"
  done
  printf 'left alone\n' >"$dir/notes.unknownext"
  touch -t 202001010101 "$dir"/*
}
hold_back() { mv "$1"/archive_* "$1.later/"; }
bring_back() { mv "$1.later"/archive_* "$1/"; }
apply() {  # apply DIR LABEL
  local out rc
  out=$("$SWEEP" apply "$1" --since 0 --yes 2>&1 </dev/null); rc=$?
  assert_eq 0 "$rc" "$2: apply exit"
}
STEP_RE="s/.*To undo it, run: //p"
sweep() { "$SWEEP" "$@"; }  # so a printed step runs exactly as printed

# --- text: the first undo says what remains, the second does not -------------
D="$W/it's a desk"
QD="'$W/it'\\''s a desk'"  # how sweep quotes it: '...it'\''s a desk'
fixture "$D"
snapshot_tree "$D" "$W/original.json"
hold_back "$D"
apply "$D" "first apply"
bring_back "$D"
snapshot_tree "$D" "$W/between.json"
apply "$D" "second apply"
assert_eq 2 "$(reversible "$D")" "status: both applies of the folder can be undone"

OUT=$("$SWEEP" undo "$D" 2>&1 </dev/null); RC=$?
assert_no_panic "undo 1" "$OUT"
assert_eq 0 "$RC" "undo 1: exit"
has "undo 1: says what it restored" "$OUT" "Restored 4 files."
LINE="Another apply of this folder can still be undone. To undo it, run: sweep undo $QD"
has "undo 1: says another apply remains, with the step" "$OUT" "$LINE"
AFTER_RESTORED=$(printf '%s\n' "$OUT" | sed -n '/^Restored /,$p')
has "undo 1: the line comes after the Restored line" "$AFTER_RESTORED" "$LINE"
hasnt "undo 1: one remains, so no count" "$OUT" "more applies"
snapshot_tree "$D" "$W/after-undo-1.json"
assert_snapshot_eq "$W/between.json" "$W/after-undo-1.json" "undo 1: the second apply is reversed"
assert_snapshot_ne "$W/original.json" "$W/after-undo-1.json" "undo 1: the folder is not yet the original"
assert_eq 1 "$(reversible "$D")" "status: one apply is left to undo"

# Run the printed step as a person would paste it.
STEP=$(printf '%s\n' "$OUT" | sed -n "$STEP_RE")
assert_eq "sweep undo $QD" "$STEP" "undo 1: the step quotes the single quote"
OUT=$(eval "$STEP" 2>&1 </dev/null); RC=$?
assert_no_panic "undo 2" "$OUT"
assert_eq 0 "$RC" "undo 2 (the printed step): exit"
has "undo 2: says what it restored" "$OUT" "Restored 8 files."
hasnt "undo 2: nothing remains, so no line" "$OUT" "can still be undone"
snapshot_tree "$D" "$W/after-undo-2.json"
assert_snapshot_eq "$W/original.json" "$W/after-undo-2.json" "undo 2: the folder equals the original"

OUT=$("$SWEEP" undo "$D" 2>&1 </dev/null); RC=$?
assert_eq 1 "$RC" "undo 3: nothing to undo, exit"
has "undo 3: says so" "$OUT" "nothing to undo"

# --- json: the count is in details, and undo with no folder says the same ----
J="$W/json"$'\t'"folder"
fixture "$J"
snapshot_tree "$J" "$W/json-original.json"
hold_back "$J"
apply "$J" "json: first apply"
bring_back "$J"
apply "$J" "json: second apply"
OUT=$("$SWEEP" undo "$J" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "tab in the name: undo 1: exit"
STEP=$(printf '%s\n' "$OUT" | sed -n "$STEP_RE")
has "tab in the name: the step uses \$'...' quoting" "$STEP" "\\x09folder'"
OUT=$(eval "$STEP" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "tab in the name: undo 2 (the printed step): exit"
hasnt "tab in the name: undo 2: nothing remains" "$OUT" "can still be undone"
snapshot_tree "$J" "$W/json-mid.json"
assert_snapshot_eq "$W/json-original.json" "$W/json-mid.json" "tab in the name: the folder equals the original"
hold_back "$J"
apply "$J" "json: third apply"
bring_back "$J"
apply "$J" "json: fourth apply"
"$SWEEP" undo "$J" --json >"$W/undo1.json" 2>/dev/null </dev/null; RC=$?
assert_eq 0 "$RC" "json undo 1: exit"
assert_eq 1 "$(field details.remaining_reversible "$W/undo1.json")" "json undo 1: one apply remains"
assert_eq done "$(field status "$W/undo1.json")" "json undo 1: status"
"$SWEEP" undo "$J" --json >"$W/undo2.json" 2>/dev/null </dev/null; RC=$?
assert_eq 0 "$RC" "json undo 2: exit"
assert_eq 0 "$(field details.remaining_reversible "$W/undo2.json")" "json undo 2: none remains"
assert_eq 0 "$(field details.records_not_opened "$W/undo2.json")" "json undo 2: every record opened"
snapshot_tree "$J" "$W/json-after.json"
assert_snapshot_eq "$W/json-original.json" "$W/json-after.json" "json: the folder equals the original"

N="$W/newest"
fixture "$N"
snapshot_tree "$N" "$W/newest-original.json"
hold_back "$N"
apply "$N" "no folder: first apply"
bring_back "$N"
apply "$N" "no folder: second apply"
OUT=$("$SWEEP" undo 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "undo with no folder 1: exit"
has "undo with no folder 1: names this folder's step" "$OUT" \
  "Another apply of this folder can still be undone. To undo it, run: sweep undo '$N'"
OUT=$("$SWEEP" undo 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "undo with no folder 2: exit"
hasnt "undo with no folder 2: nothing remains, so no line" "$OUT" "can still be undone"
snapshot_tree "$N" "$W/newest-after.json"
assert_snapshot_eq "$W/newest-original.json" "$W/newest-after.json" "no folder: the folder equals the original"

# --- a byte that is not UTF-8 in the folder name ------------------------------
BAD="$W/bad$(printf '\377')name"
if mkdir "$BAD" 2>/dev/null; then
  fixture "$BAD"
  snapshot_tree "$BAD" "$W/bad-original.json"
  hold_back "$BAD"
  apply "$BAD" "not UTF-8: first apply"
  bring_back "$BAD"
  apply "$BAD" "not UTF-8: second apply"
  OUT=$("$SWEEP" undo "$BAD" 2>&1 </dev/null); RC=$?
  assert_eq 0 "$RC" "not UTF-8: undo 1: exit"
  STEP=$(printf '%s\n' "$OUT" | sed -n "$STEP_RE")
  has "not UTF-8: the step names the byte" "$STEP" "\\xff"
  OUT=$(eval "$STEP" 2>&1 </dev/null); RC=$?
  assert_eq 0 "$RC" "not UTF-8: undo 2 (the printed step): exit"
  snapshot_tree "$BAD" "$W/bad-after.json"
  assert_snapshot_eq "$W/bad-original.json" "$W/bad-after.json" "not UTF-8: the folder equals the original"
else
  unproven "a folder name with a byte that is not UTF-8" "this filesystem refuses such a name"
fi
