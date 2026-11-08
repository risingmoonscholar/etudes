#!/usr/bin/env bash
# A name a person typed is shown as typed, not as Rust debug text. A folder
# called Blend"er x used to print as "Blend\"er x" in refusals. These refusals
# show the name between no marks at all, say nothing moved, and leave the tree
# as it was. Control characters in a name still print as '?'. Then a stash and
# pop and an unpack round trip still work with the same awkward name.
# Everything is synthetic, in a temp dir, and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null
require python3 "checks the tree before and after" || exit 0

W=$(workdir)
# Only a fresh temp folder is ever removed or used as a working folder.
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
mkdir -p "$HOME" "$ETUDE_STATE_DIR" "$W/d"
printf 'x' >"$W/d/a.bpy"
printf 'y' >"$W/d/b.xyz"
snapshot_tree "$W/d" "$W/before.txt"

NAME='Blend"er x'
refuses() {  # refuses LABEL WANT TOOL ARGS...: exit 2, exact text, nothing moved
  local label="$1" want="$2"; shift 2
  local out code
  out=$("$@" 2>&1); code=$?
  assert_no_panic "$label" "$out"
  assert_eq 2 "$code" "$label: exit"
  case "$out" in
    *"$want"*) pass "$label: shows the name as typed" ;;
    *) fail "$label: wanted '$want' in: $(printf '%s' "$out" | head -c 400)" ;;
  esac
  case "$out" in
    *'\"'*) fail "$label: still shows a backslash-escaped quote" ;;
    *) pass "$label: no escaped quote" ;;
  esac
  snapshot_tree "$W/d" "$W/after.txt"
  assert_snapshot_eq "$W/before.txt" "$W/after.txt" "$label: nothing moved"
}

refuses "sweep two extensions to one folder" \
  "two extensions map to Blend\"er x; each mapping needs its own folder" \
  "$SWEEP" "$W/d" --since 0 --map "bpy=$NAME" --map "xyz=$NAME"
refuses "sweep hidden folder name" \
  '--map: .hid"den x is not a plain visible folder name' \
  "$SWEEP" "$W/d" --since 0 --map 'bpy=.hid"den x'
refuses "sweep bad extension" \
  '--map: a/b "c" is not a single extension (like bpy)' \
  "$SWEEP" "$W/d" --since 0 --map 'a/b "c"=Box'
refuses "sweep control character in an extension" \
  '--map: a/b?"c" is not a single extension (like bpy)' \
  "$SWEEP" "$W/d" --since 0 --map "$(printf 'a/b\n"c"')=Box"
refuses "stash duration" \
  'stash: cannot read duration 5 "x". Try 30m, 2h, 3d or 1w.' \
  "$STASH" "$W/d" --for '5 "x"'

# Stash and pop with the same awkward name still round-trip.
S="$W/$NAME"; mkdir -p "$S"
printf 'one' >"$S/one.txt"; printf 'two' >"$S/two.txt"
snapshot_tree "$S" "$W/s-before.txt"
OUT=$("$STASH" "$S" 2>&1); CODE=$?
assert_no_panic "stash" "$OUT"
assert_eq 0 "$CODE" "stash clears a folder named with a quote and a space (exit $CODE)"
OUT=$("$STASH" pop "$S" 2>&1); CODE=$?
assert_no_panic "stash pop" "$OUT"
assert_eq 0 "$CODE" "pop restores them (exit $CODE)"
snapshot_tree "$S" "$W/s-after.txt"
assert_snapshot_eq "$W/s-before.txt" "$W/s-after.txt" "the folder is as it was after pop"

# Unpack to a folder with that name extracts the exact bytes.
SRC="$W/src"; mkdir -p "$SRC"
printf 'synthetic member\n' >"$SRC/member.txt"
tar -czf "$W/fixture.tar.gz" -C "$SRC" member.txt
OUT=$("$UNPACK" "$W/fixture.tar.gz" --into "$W/out $NAME" 2>&1); CODE=$?
assert_no_panic "unpack" "$OUT"
assert_eq 0 "$CODE" "unpack extracts into a folder named with a quote and a space (exit $CODE)"
[ "$(cat "$W/out $NAME/member.txt" 2>/dev/null)" = 'synthetic member' ] \
  && pass "unpack wrote the member with its exact bytes" \
  || fail "unpack did not write the exact bytes"
