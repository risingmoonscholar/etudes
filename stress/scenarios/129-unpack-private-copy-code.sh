#!/usr/bin/env bash
# unpack declaration 9 adds the refusal code private_copy_unopened: the system
# unzip could not open unpack's private copy of the archive on any attempt. The
# archive was not read, so the code asks for the same request again later. It
# lives in the shared reason table, so this checks that unpack lists it as
# declared, that sweep and stash do not, and that all three tools still run.
# A damaged zip must still be archive_unreadable: the new code must not
# swallow real damage. The race itself is proven by unpack's unit tests and by
# scenario 98. Everything is synthetic, in a temp dir, and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null
require python3 "reads the JSON results" || exit 0
require zip "the fixture needs zip" || exit 0

W=$(workdir) || exit 1
case "$W" in
  "${TMPDIR:-/tmp}"/*|/private/var/folders/*|/tmp/*|/private/tmp/*) ;;
  *) echo "refusing to use $W" >&2; exit 1 ;;
esac
[ -d "$W" ] || { echo "workdir missing: $W" >&2; exit 1; }
cleanup() {
  case "$W" in
    "${TMPDIR:-/tmp}"/*|/private/var/folders/*|/tmp/*|/private/tmp/*) rm -rf "$W" ;;
  esac
  eval "$_stress_state_cleanup"
}
trap cleanup EXIT
cd "$W" || exit 1
export HOME="$W/home" ETUDE_STATE_DIR="$W/state"
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets;print(secrets.token_hex(32))')"
mkdir -p "$HOME" "$ETUDE_STATE_DIR"

# field FILE EXPRESSION: evaluate a Python expression over the parsed JSON `r`.
field() {
  python3 - "$1" "$2" <<'PY'
import json, sys
r = json.load(open(sys.argv[1]))
print(eval(sys.argv[2], {"r": r}))
PY
}

# --- the contract ------------------------------------------------------------
"$UNPACK" contract --json >"$W/unpack-contract.json" 2>"$W/err"
assert_eq 0 "$?" "unpack contract exits 0"
assert_eq 9 "$(field "$W/unpack-contract.json" 'r["details"]["schema_version"]')" \
  "unpack declares version 9"
ROW='[c for c in r["details"]["refusal_codes"] if c["code"] == "private_copy_unopened"]'
assert_eq 1 "$(field "$W/unpack-contract.json" "len($ROW)")" \
  "unpack lists private_copy_unopened once"
assert_eq "later input extract,list" \
  "$(field "$W/unpack-contract.json" "' '.join([$ROW[0]['retry'], $ROW[0]['subject_kind'], ','.join($ROW[0]['operations'])])")" \
  "private_copy_unopened is retry later, subject input, for extract and list"
for pair in "sweep:$SWEEP" "stash:$STASH"; do
  tool=${pair%%:*}; bin=${pair#*:}
  "$bin" contract --json >"$W/$tool-contract.json" 2>"$W/err"
  assert_eq 0 "$?" "$tool contract exits 0"
  assert_eq 0 "$(field "$W/$tool-contract.json" "len([c for c in r['details']['refusal_codes'] if c['code'] == 'private_copy_unopened'])")" \
    "$tool does not list private_copy_unopened"
done

# --- unpack still extracts, and damage is still damage -----------------------
mkdir -p "$W/src"
printf 'plain' >"$W/src/plain.txt"
(cd "$W/src" && zip -q "$W/input.zip" plain.txt)
"$UNPACK" "$W/input.zip" --into "$W/out" --json >"$W/unpack.json" 2>"$W/err"
assert_eq 0 "$?" "unpack extracts a safe zip"
assert_eq done "$(field "$W/unpack.json" 'r["reason"]["code"]')" "unpack's reason is done"
assert_eq plain "$(cat "$W/out/plain.txt" 2>/dev/null)" "the extracted file has its bytes"
"$UNPACK" "$W/input.zip" --list --json >"$W/list.json" 2>"$W/err"
assert_eq 0 "$?" "unpack --list reads a safe zip"
printf 'this is not a zip archive' >"$W/bad.zip"
out=$("$UNPACK" "$W/bad.zip" --json 2>"$W/bad.err"); code=$?
printf '%s' "$out" >"$W/bad.json"
assert_eq 3 "$code" "a damaged zip exits 3"
assert_eq "archive_unreadable after_change" \
  "$(field "$W/bad.json" 'r["reason"]["code"] + " " + r["reason"]["retry"]')" \
  "a damaged zip is still archive_unreadable, not private_copy_unopened"
assert_no_panic "damaged zip" "$(cat "$W/bad.err")"
[ ! -e "$W/bad" ] && pass "a damaged zip leaves no folder" || fail "a damaged zip left a folder"

# --- sweep and stash still run ----------------------------------------------
mkdir -p "$W/d"
for n in 1 2 3; do printf "shot $n" >"$W/d/shot-$n.png"; done
"$SWEEP" "$W/d" --since 0 --json >"$W/sweep.json" 2>"$W/err"
assert_eq 0 "$?" "sweep plans a folder"
snapshot_tree "$W/d" "$W/before.txt"
"$STASH" "$W/d" --for 1d --json >"$W/stash.json" 2>"$W/err"
assert_eq 0 "$?" "stash holds the folder's files"
"$STASH" pop "$W/d" --json >"$W/pop.json" 2>"$W/err"
assert_eq 0 "$?" "stash pop brings them back"
snapshot_tree "$W/d" "$W/after.txt"
assert_snapshot_eq "$W/before.txt" "$W/after.txt" "stash and pop leave the folder as it was"
