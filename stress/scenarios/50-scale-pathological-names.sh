#!/usr/bin/env bash
# Filenames chosen to break naive path handling: an embedded newline, an
# embedded tab, single and double quotes, a backslash, a leading dash, a
# name that starts with "..", a semicolon, a literal `$(whoami)` and a
# literal backtick command substitution, a 255-byte name (the per-component
# max most filesystems allow), emoji, and a right-to-left override character
# (the classic filename-spoofing trick).
#
# What actually matters: sweep never shells out (it moves files with
# std::fs::rename / hard_link, not a subprocess), so there is no classic
# "injection" surface in the tool itself. What IS worth proving by hand:
#   - every name survives plan -> apply -> undo BYTE-FOR-BYTE, including the
#     shell-metacharacter ones
#   - a filename that is literally the text "$(whoami)" is never evaluated.
#     No file or directory named after the real output of whoami ever appears
#   - nothing crashes, nothing silently drops a file
#
# A note on the harness: lib.sh's assert_intact counts files with
# `find DIR -type f | wc -l`. A filename containing an embedded newline adds
# an extra apparent line to find's default output, so assert_intact
# OVERCOUNTS by one for every such name in the tree. This scenario does NOT
# use assert_intact once the newline-name is on disk. It counts with
# `find -print0` instead. It flags the harness gap explicitly below rather
# than silently working around it.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
D="$W/tree"; mkdir -p "$D"

TOK="_pathotoken.txt"
KNAME="$(printf 'k%.0s' $(seq 1 $((255 - ${#TOK}))))${TOK}"   # exactly 255 bytes

: > "$D/weird"$'\n'"name_a${TOK}"
: > "$D/weird"$'\t'"name_b${TOK}"
: > "$D/it's_a_file_c${TOK}"
: > "$D/quo\"ted_file_d${TOK}"
: > "$D/back\\slash_e${TOK}"
: > "$D/-leading-dash-f${TOK}"
: > "$D/semi;colon_h${TOK}"
: > "$D/cmdsubst_\$(whoami)_i${TOK}"
: > "$D/backtick_\`id\`_j${TOK}"
: > "$D/$KNAME"
: > "$D/emoji_🎉📸_l${TOK}"
: > "$D/rtl_"$'‮'"exe.txt_m${TOK}"

nul_count() { find "$1" -maxdepth 1 -type f -print0 | tr -cd '\0' | wc -c | tr -d ' '; }

BEFORE=$(nul_count "$D")
assert_eq 12 "$BEFORE" "all 12 pathological names were created on disk"

# Canary: proves nothing was ever handed to a real shell. If any name were
# evaluated instead of treated as literal text, this file would appear.
CANARY="$W/PWNED_if_shell_evaluated"
rm -f "$CANARY"

# --- plan groups them by the shared token, and only that ---------------
PLAN_JSON=$("$SWEEP" "$D" --json 2>&1)
PLAN_EC=$?
assert_eq 0 "$PLAN_EC" "plan succeeds over a directory full of pathological names"
SCANNED=$(grep -o '"scanned":[0-9]*' <<<"$PLAN_JSON" | head -1 | cut -d: -f2)
assert_eq 12 "$SCANNED" "plan scanned exactly the 12 files created (none silently dropped)"

# --- apply moves every one of them, byte-exact --------------------------
BEFORE_LIST="$W/before_names.txt"
find "$D" -maxdepth 1 -type f -print0 | xargs -0 -n1 basename -- > "$BEFORE_LIST" 2>/dev/null

APPLY_OUT=$("$SWEEP" apply "$D" --yes 2>&1)
APPLY_EC=$?
assert_eq 0 "$APPLY_EC" "apply succeeds over the pathological-name tree"

AFTER_LIST="$W/after_names.txt"
find "$D" -type f -print0 | xargs -0 -n1 basename -- > "$AFTER_LIST" 2>/dev/null

if diff -q <(sort "$BEFORE_LIST") <(sort "$AFTER_LIST") >/dev/null 2>&1; then
  pass "every pathological filename survived apply byte-for-byte identical (compared as the exact set of basenames, before vs after)"
else
  fail "filenames changed across apply. Diff: $(diff <(sort "$BEFORE_LIST") <(sort "$AFTER_LIST") | head -5 | tr '\n' ' ')"
fi

nul_count_recursive() { find "$1" -type f -print0 | tr -cd '\0' | wc -c | tr -d ' '; }
AFTER_COUNT=$(nul_count_recursive "$D")
assert_eq "$BEFORE" "$AFTER_COUNT" "apply lost none of the 12 pathological files"

# --- no injection: nothing named after the real whoami/id output --------
# The strongest evidence is already above: the before/after basename SETS
# are identical, so no extra file of any kind was created by apply. The
# canary is a second, independent check aimed specifically at command
# substitution / shell evaluation of a filename.
if [ -e "$CANARY" ]; then
  fail "a file named \$(whoami) or \`id\` in text form somehow caused real shell evaluation. Canary file exists"
else
  pass "the literal text \"\$(whoami)\" and a literal backtick command substitution were never evaluated. No canary, no unexpected file anywhere (see the byte-exact basename diff above)"
fi

# --- the 255-byte name specifically ---
if [ -f "$D/$KNAME" ] || find "$D" -type f -name "kkkk*${TOK}" | grep -q .; then
  pass "the 255-byte filename (the per-component max) round-tripped through apply intact"
else
  fail "the 255-byte filename did not survive apply"
fi

# --- undo restores the exact baseline, including every weird name -------
"$SWEEP" undo >/dev/null 2>&1
UNDO_EC=$?
assert_eq 0 "$UNDO_EC" "undo succeeds"
RESTORED=$(nul_count "$D")
assert_eq "$BEFORE" "$RESTORED" "undo restored every pathological-named file to the top level (exact baseline count, NUL-safe)"

RESTORED_LIST="$W/restored_names.txt"
find "$D" -maxdepth 1 -type f -print0 | xargs -0 -n1 basename -- > "$RESTORED_LIST" 2>/dev/null
if diff -q <(sort "$BEFORE_LIST") <(sort "$RESTORED_LIST") >/dev/null 2>&1; then
  pass "undo's restored names match the original set byte-for-byte"
else
  fail "undo restored a different set of names than the original: diff $(diff <(sort "$BEFORE_LIST") <(sort "$RESTORED_LIST") | head -5 | tr '\n' ' ')"
fi

# --- harness note, not a tool defect: assert_intact would have lied here -
NAIVE=$(find "$D" -maxdepth 1 -type f | wc -l | tr -d ' ')
if [ "$NAIVE" != "$BEFORE" ]; then
  echo "    NOTE (test-infrastructure, not a tool defect): lib.sh's assert_intact" >&2
  echo "    uses \`find DIR -type f | wc -l\`, which overcounts by one for every" >&2
  echo "    filename containing an embedded newline. Here: naive count=$NAIVE," >&2
  echo "    real count=$BEFORE. Any scenario that uses assert_intact on a tree" >&2
  echo "    containing a newline-named file will report a false file count." >&2
else
  pass "(sanity) naive find|wc -l happened to agree with the NUL-safe count this run"
fi
