#!/usr/bin/env bash
# A stash older than sweep's 30-day expiry keeps its undo record: a sweep run
# drops an old sweep record but not the stash's, and pop brings every file back
# with its bytes. Once popped and 30 days old, the stash's record expires too.
# A scan holds no key and prunes nothing; an apply or undo prunes with its own
# key. An old sweep record that another key cannot open is kept through an
# apply and a wrong-key undo under that key, and the right key then undoes it.
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

# A scan holds no key, so it prunes nothing.
"$SWEEP" "$W/scan" >/dev/null 2>"$W/scan.err"
assert_eq 1 "$?" "a sweep scan of an empty folder runs (nothing to organise) with both records past 30 days"
if [ -f "$ETUDE_STATE_DIR/$stash_record" ] && [ -f "$ETUDE_STATE_DIR/$sweep_record" ]; then pass "a scan drops no record"; else fail "a scan, which holds no key, deleted a record"; fi
# With no key supplied, a scan must not reach for the keychain at all.
if keychain_fence_works; then
  keychain_fenced "$SWEEP" "$W/scan" --json >"$W/fenced.json" 2>"$W/fenced.err"
  assert_eq 1 "$?" "a scan with no key supplied runs inside the keychain fence"
  if python3 -c 'import json,sys; c=[x["category"] for x in json.load(open(sys.argv[1]))["observations"]["categories"]]; sys.exit("key_material" in c)' "$W/fenced.json" && [ -f "$ETUDE_STATE_DIR/$sweep_record" ]; then pass "a scan with an old record reads no key and keeps the record"; else fail "a keyless scan read key material or dropped a record: $(cat "$W/fenced.err")"; fi
else
  unproven 'keyless scan reads no key' 'sandbox-exec could not block /usr/bin/security here, and this suite never reaches the real keychain'
fi
# An apply holds the key, and drops old records it can open before it moves.
mkdir -p "$W/swept2"
for n in 1 2 3; do printf 'two %s' "$n" >"$W/swept2/Screenshot 2024-01-03 at 00.00.0$n.png"; done
touch -t 202401030000 "$W/swept2"/*.png
"$SWEEP" apply "$W/swept2" --yes --since 0 >/dev/null 2>"$W/apply2.err"
assert_eq 0 "$?" "a second apply runs with both records past 30 days"
if [ -f "$ETUDE_STATE_DIR/$stash_record" ]; then pass "the outstanding stash record survives sweep's prune"; else fail "sweep's prune deleted a stash record whose files are still held"; fi
if [ ! -e "$ETUDE_STATE_DIR/$sweep_record" ]; then pass "the old sweep record is dropped by the next apply"; else fail "an old sweep record survived an apply holding its key"; fi
if grep -q 'dropped 1 undo record older than 30 days' "$W/apply2.err"; then pass "sweep says it dropped one record"; else fail "sweep's prune message: $(cat "$W/apply2.err")"; fi
if [ "$(ls "$ETUDE_STATE_DIR" | grep -c '^sweep-.*\.journal$')" = 1 ]; then pass "the apply's own new record is kept"; else fail "the apply's own record is missing"; fi

"$STASH" pop "$W/held" --json >"$W/pop.json" 2>"$W/pop.err"
assert_eq 0 "$?" "pop of the 40-day-old stash succeeds"
if python3 -c 'import json,sys; c=json.load(open(sys.argv[1]))["recovery"]["conditions"]; sys.exit(not (c and not any("expired" in x for x in c)))' "$W/pop.json"; then pass "pop's recovery conditions do not claim a stash record expires"; else fail "pop's recovery conditions: $(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["recovery"])' "$W/pop.json")"; fi
(cd "$W/held" && find . -type f -print0 | sort -z | xargs -0 shasum -a 256) >"$W/after.sum"
if cmp -s "$W/before.sum" "$W/after.sum"; then pass "every stashed file is back with its bytes"; else fail "restored files differ: $(diff "$W/before.sum" "$W/after.sum" | head -5)"; fi
if ls -A "$W/held" | grep -q '^\.stash-'; then fail "a holding folder is left after pop"; else pass "no holding folder is left"; fi

# Popped, then 30 days pass: the record holds nothing and expires.
touch -t "$OLD" "$ETUDE_STATE_DIR/$stash_record"
"$SWEEP" undo "$W/swept2" >/dev/null 2>&1
if [ -f "$ETUDE_STATE_DIR/$stash_record" ]; then pass "sweep's prune leaves even a popped stash record to stash"; else fail "sweep's prune deleted a stash record"; fi
stash_id=${stash_record#stash-}; stash_id=${stash_id%.journal}
"$STASH" pop --id "$stash_id" >"$W/pop2.out" 2>"$W/pop2.err"
assert_eq 1 "$?" "pop by id of the old popped stash exits 1"
if grep -q 'already popped' "$W/pop2.out"; then pass "it still answers already popped, then drops the record"; else fail "pop by id of an old popped stash: $(cat "$W/pop2.out" "$W/pop2.err")"; fi
if [ ! -e "$ETUDE_STATE_DIR/$stash_record" ]; then pass "the popped stash record expires once 30 days old"; else fail "a popped stash record outlived its 30 days"; fi
if grep -q 'dropped 1 undo record older than 30 days' "$W/pop2.err"; then pass "stash says it dropped one record"; else fail "stash's prune message: $(cat "$W/pop2.err")"; fi

# A sweep record that does not open with the current key is kept past 30
# days, so "Keep that record" is true, and the right key still undoes it.
mkdir -p "$W/keyed"
for n in 1 2 3; do printf 'keyed %s' "$n" >"$W/keyed/Screenshot 2024-01-02 at 00.00.0$n.png"; done
touch -t 202401020000 "$W/keyed"/*.png
(cd "$W/keyed" && find . -type f -print0 | sort -z | xargs -0 shasum -a 256) >"$W/keyed.sum"
"$SWEEP" apply "$W/keyed" --yes --since 0 >/dev/null 2>&1
assert_eq 0 "$?" "sweep apply under the first key"
keyed_record=$(ls -t "$ETUDE_STATE_DIR" | grep '^sweep-.*\.journal$' | head -1)
touch -t "$OLD" "$ETUDE_STATE_DIR/$keyed_record"
OTHER_KEY=$(python3 -c 'import secrets; print(secrets.token_hex(32))')
mkdir -p "$W/other"
for n in 1 2 3; do printf 'other %s' "$n" >"$W/other/Screenshot 2024-01-04 at 00.00.0$n.png"; done
touch -t 202401040000 "$W/other"/*.png
ETUDE_JOURNAL_KEY=$OTHER_KEY "$SWEEP" apply "$W/other" --yes --since 0 >/dev/null 2>&1
assert_eq 0 "$?" "an apply under another key runs"
if [ -f "$ETUDE_STATE_DIR/$keyed_record" ]; then pass "an old record another key cannot open survives that apply's prune"; else fail "sweep's prune deleted a record it could not open"; fi
keyed_id=${keyed_record#sweep-}; keyed_id=${keyed_id%.journal}
ETUDE_JOURNAL_KEY=$OTHER_KEY "$SWEEP" undo --id "$keyed_id" >/dev/null 2>"$W/wrongkey.err"
assert_eq 3 "$?" "undo with the wrong key refuses"
if grep -q 'Keep that record' "$W/wrongkey.err" && [ -f "$ETUDE_STATE_DIR/$keyed_record" ]; then pass "undo says keep the record, and it is still there"; else fail "wrong-key undo: $(cat "$W/wrongkey.err")"; fi
# By id: the newer record from the other key would stop a folder undo first.
"$SWEEP" undo --id "$keyed_id" >/dev/null 2>"$W/rightkey.err"
assert_eq 0 "$?" "undo with the right key succeeds after 30 days"
(cd "$W/keyed" && find . -type f -print0 | sort -z | xargs -0 shasum -a 256) >"$W/keyed.after"
if cmp -s "$W/keyed.sum" "$W/keyed.after"; then pass "the right key brings every file back with its bytes"; else fail "files after undo differ: $(diff "$W/keyed.sum" "$W/keyed.after" | head -5)"; fi

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
