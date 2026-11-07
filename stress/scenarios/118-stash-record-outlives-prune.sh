#!/usr/bin/env bash
# A stash older than sweep's 30-day expiry keeps its undo record: a sweep run
# drops an old sweep record but not the stash's, and pop brings every file back
# with its bytes. Once popped and 30 days old, the stash's record expires too.
# Everything is synthetic, in a temp dir, and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null
require python3 "builds the fixture tree" || exit 0

W=$(workdir)
cleanup() {
  rm -rf "$W"
  eval "$_stress_state_cleanup"
}
trap cleanup EXIT
export HOME="$W/home"
mkdir -p "$HOME" "$W/held" "$W/swept" "$W/scan"
OLD=$(date -v-40d +%Y%m%d%H%M)

python3 - "$W/held" <<'PY'
import pathlib, sys
root = pathlib.Path(sys.argv[1])
for n in range(12):
    (root/f'item-{n:02}.bin').write_bytes(bytes((n * 7 + i) % 256 for i in range(4096 + n)))
PY
(cd "$W/held" && find . -type f -print0 | sort -z | xargs -0 shasum -a 256) >"$W/before.sum"
for n in 1 2 3; do printf 'shot %s' "$n" >"$W/swept/Screenshot 2024-01-01 at 00.00.0$n.png"; done
touch -t 202401010000 "$W/swept"/*.png

"$STASH" "$W/held" --for 6w >/dev/null 2>"$W/stash.err"
assert_eq 0 "$?" "stash takes the folder"
"$SWEEP" apply "$W/swept" --yes --since 0 >/dev/null 2>&1
assert_eq 0 "$?" "sweep apply writes a sweep record"
stash_record=$(ls "$ETUDE_STATE_DIR" | grep '^stash-.*\.journal$')
sweep_record=$(ls "$ETUDE_STATE_DIR" | grep '^sweep-.*\.journal$')
touch -t "$OLD" "$ETUDE_STATE_DIR/$stash_record" "$ETUDE_STATE_DIR/$sweep_record"

"$SWEEP" "$W/scan" >/dev/null 2>"$W/scan.err"
assert_eq 1 "$?" "a sweep scan of an empty folder runs (nothing to organise) with both records past 30 days"
if [ -f "$ETUDE_STATE_DIR/$stash_record" ]; then pass "the outstanding stash record survives sweep's prune"; else fail "sweep's prune deleted a stash record whose files are still held"; fi
if [ ! -e "$ETUDE_STATE_DIR/$sweep_record" ]; then pass "the old sweep record is still dropped"; else fail "an old sweep record survived its 30 days"; fi
if grep -q 'dropped 1 undo record older than 30 days' "$W/scan.err"; then pass "sweep says it dropped one record"; else fail "sweep's prune message: $(cat "$W/scan.err")"; fi

"$STASH" pop "$W/held" >/dev/null 2>"$W/pop.err"
assert_eq 0 "$?" "pop of the 40-day-old stash succeeds"
(cd "$W/held" && find . -type f -print0 | sort -z | xargs -0 shasum -a 256) >"$W/after.sum"
if cmp -s "$W/before.sum" "$W/after.sum"; then pass "every stashed file is back with its bytes"; else fail "restored files differ: $(diff "$W/before.sum" "$W/after.sum" | head -5)"; fi
if ls -A "$W/held" | grep -q '^\.stash-'; then fail "a holding folder is left after pop"; else pass "no holding folder is left"; fi

# Popped, then 30 days pass: the record holds nothing and expires.
touch -t "$OLD" "$ETUDE_STATE_DIR/$stash_record"
"$SWEEP" "$W/scan" >/dev/null 2>&1
if [ -f "$ETUDE_STATE_DIR/$stash_record" ]; then pass "sweep's keyless prune leaves the popped stash record to stash"; else fail "sweep's prune deleted a stash record"; fi
"$STASH" pop "$W/scan" >/dev/null 2>"$W/pop2.err"
assert_eq 1 "$?" "pop of a folder with nothing stashed exits 1"
if [ ! -e "$ETUDE_STATE_DIR/$stash_record" ]; then pass "the popped stash record expires once 30 days old"; else fail "a popped stash record outlived its 30 days"; fi
if grep -q 'dropped 1 undo record older than 30 days' "$W/pop2.err"; then pass "stash says it dropped one record"; else fail "stash's prune message: $(cat "$W/pop2.err")"; fi

python3 - "$W" <<'PY'
import pathlib, sys, zipfile
w = pathlib.Path(sys.argv[1])
src = w/'zipsrc'; src.mkdir()
(src/'member.txt').write_bytes(b'archive member')
(src/'second.txt').write_bytes(b'second member')
with zipfile.ZipFile(w/'small.zip', 'w') as z:
    z.write(src/'member.txt', 'member.txt')
    z.write(src/'second.txt', 'second.txt')
PY
"$UNPACK" "$W/small.zip" --into "$W/out" >/dev/null 2>&1
assert_eq 0 "$?" "unpack extracts a small zip"
assert_eq "archive member" "$(cat "$W/out/member.txt" 2>/dev/null)" "the extracted member has its bytes"
