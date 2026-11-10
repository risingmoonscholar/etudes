#!/usr/bin/env bash
# APFS stores filenames exactly as given (case-preserving) but compares them
# for normalization, not just case: "café" typed as one precomposed
# character (NFC, U+00E9) and "café" typed as e + a combining acute accent
# (NFD, U+0065 U+0301) are visually identical and refer to the SAME
# directory entry if they ever land in the same folder. This is how
# filenames typed on Linux/Windows (usually NFC) collide with names that
# passed through a macOS Finder rename or an NFD-normalizing tool.
#
# The sibling scenario 20-case-fold-collision.sh proves sweep's collision
# check folds ASCII case correctly. This one asks the same question one
# normalization form over.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

if ! require python3 "constructs byte-exact NFC vs NFD filenames"; then
  exit 0
fi

W=$(workdir) || exit 1; trap 'detach_registered_mounts; rm -rf "$W"' EXIT
D="$W/Desktop"
mkdir -p "$D/ClientA" "$D/ClientB"

python3 - "$D" <<'PYEOF'
import sys, os, unicodedata
d = sys.argv[1]
nfc = unicodedata.normalize('NFC', 'invoice_café.pdf')  # single U+00E9
nfd = unicodedata.normalize('NFD', 'invoice_café.pdf')  # e + U+0301
assert nfc != nfd, "test setup broken: forms are byte-identical"
with open(os.path.join(d, 'ClientA', nfc), 'w') as f:
    f.write('ORIGINAL-CONTENT-FROM-CLIENT-A\n')
with open(os.path.join(d, 'ClientB', nfd), 'w') as f:
    f.write('ORIGINAL-CONTENT-FROM-CLIENT-B\n')
for i in range(3):
    with open(os.path.join(d, f'invoice_filler_{i}.pdf'), 'w') as f:
        f.write('filler\n')
PYEOF

BEFORE=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq 5 "$BEFORE" "fixture built: 5 files, two of them the same visible name, one NFC one NFD"

assert_exit 0 "planning a tree with an NFC/NFD name pair succeeds" -- "$SWEEP" "$D" --depth 2

# Both land in the Documents group: they are .pdf files, and that is what
# they are. This used to key on a shared "invoice" token; that rule is gone.
# Scanning both is right -- they are genuinely two different directory
# entries, in different subdirectories.
group_count=$("$SWEEP" "$D" --depth 2 --json | python3 -c "
import json, sys
d = json.load(sys.stdin)['details']
g = [g for g in d['groups'] if g['name'] == 'Documents']
print(g[0]['count'] if g else 0)
")
assert_eq 5 "$group_count" "both NFC and NFD entries were scanned and grouped (not double-counted, not silently merged)"

# The moment that matters: apply. sweep's own contract for two members of an
# accepted group whose destination basenames collide is a clean pre-flight
# refusal: DestinationCollision, exit 2, nothing moved. That is exactly
# what happens for a pure ASCII-case collision (see the sibling scenario).
snapshot_tree "$D" "$W/before-refusal.json"
apply_out=$("$SWEEP" apply "$D" --depth 2 --yes 2>&1)
apply_code=$?

a_survived=0; [ -f "$D/ClientA"/*.pdf ] 2>/dev/null && a_survived=1
b_survived=0; [ -f "$D/ClientB"/*.pdf ] 2>/dev/null && b_survived=1

if [ "$apply_code" = "2" ] && [ "$a_survived" = "1" ] && [ "$b_survived" = "1" ]; then
  pass "NFC/NFD collision refused cleanly before any move, like the ASCII-case case"
else
  fail "REAL DEFECT: the destination-collision check does not fold Unicode \
normalization. apply exited $apply_code (wanted 2/DestinationCollision) with: \
${apply_out%%$'\n'*}. ClientA survived=$a_survived ClientB survived=$b_survived. \
apply.rs's collision guard keys planned_destinations on \
dst.to_string_lossy().to_lowercase(), which folds case but does not \
normalize composition, so the NFC and NFD destination strings are treated \
as two distinct, non-colliding paths in memory even though APFS resolves \
them to one directory entry. The result: the first of the pair actually \
moves for real, then the second's move fails against the filesystem \
(EEXIST) mid-run. not the clean all-or-nothing refusal sweep promises \
everywhere else. The tree is left in a partially-applied state and the \
user must know to run 'sweep undo'."
fi

snapshot_tree "$D" "$W/after-refusal.json"
assert_snapshot_eq "$W/before-refusal.json" "$W/after-refusal.json" "Unicode preflight refusal preserves every path and byte"

# Confirm the source and destination originals remain byte-for-byte intact.
survivor_content=""
for f in "$D"/ClientA/*.pdf "$D"/ClientB/*.pdf "$D"/Documents/*.pdf; do
  [ -f "$f" ] || continue
  survivor_content="$survivor_content$(cat "$f")"$'\n'
done
if [[ "$survivor_content" == *"CLIENT-A"* ]] && [[ "$survivor_content" == *"CLIENT-B"* ]]; then
  pass "no byte content was destroyed. both originals are recoverable somewhere on disk"
else
  fail "DATA LOSS: only one of the two original file contents can be found after apply. A=$a_survived B=$b_survived, surviving text: $survivor_content"
fi

# A clean preflight refusal creates no journal, so undo must remain a no-op.
UNDO_OUT=$("$SWEEP" undo 2>&1)
UNDO_EC=$?
assert_eq 1 "$UNDO_EC" "undo has no journal to reverse after the preflight refusal"
AFTER=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq "$BEFORE" "$AFTER" "the refusal left the entire original file count in place"

# The former cargo-only witness did not exercise the release binary. Put
# source files on a second device and introduce an NFD destination alias
# only after preflight, with apply stopped and the NFC source still pending.
cross_device_alias() {
  local outer="$W/cross" inner="$W/cross/inner" image="$W/alias.dmg" target pid state code out
  if ! command -v hdiutil >/dev/null || ! create_disk_image "$image" 64m APFS UnicodeAlias; then
    unproven "cross-device NFC/NFD alias refusal" "cannot create an APFS image here"
    return
  fi
  mkdir -p "$inner"
  if ! hdiutil attach "$image" -mountpoint "$inner" -nobrowse >/dev/null 2>&1; then
    unproven "cross-device NFC/NFD alias refusal" "cannot attach the APFS image here"
    return
  fi
  register_mount "$inner"
  if [ "$(stat -f %d "$inner")" = "$(stat -f %d "$outer")" ]; then
    unproven "cross-device NFC/NFD alias refusal" "source and destination have the same device"
    return
  fi
  python3 - "$inner" <<'PY_CROSS'
import pathlib, sys
root = pathlib.Path(sys.argv[1])
for i in range(500):
    # Letter-only names avoid numeric private-record markers.
    name = ''.join(chr(97 + (i // 26**n) % 26) for n in (2, 1, 0))
    (root / f'file_{name}.pdf').write_bytes(b'filler')
(root / 'zz_invoice_café.pdf').write_bytes(b'cross-device original')
PY_CROSS
  target="$(cd "$inner" && pwd -P)/zz_invoice_café.pdf"
  "$SWEEP" apply "$outer" --depth 2 --yes >"$W/cross.out" 2>&1 & pid=$!
  if ! "$(dirname "$SWEEP")/journal-dump" --watch-apply "$ETUDE_STATE_DIR" "$outer" "$target" "$pid" >"$W/cross.witness" 2>"$W/cross.watch.err"; then
    kill -CONT "$pid" 2>/dev/null || true
    wait "$pid"; code=$?
    assert_eq 0 "$code" "unwitnessed cross-device apply succeeds"
    unproven "cross-device NFC/NFD alias refusal" "no stopped apply with pending NFC source"
    return
  fi
  state=$(ps -o state= -p "$pid")
  if [[ "$state" != *T* ]] || [ ! -f "$target" ] || [ ! -d "$outer/Documents" ]; then
    fail "cross-device alias watcher did not retain a stopped pending source"
    kill -CONT "$pid"; wait "$pid" 2>/dev/null || true
    return
  fi
  python3 - "$outer/Documents" <<'PY_ALIAS'
import pathlib, sys, unicodedata
path = pathlib.Path(sys.argv[1]) / unicodedata.normalize('NFD', 'zz_invoice_café.pdf')
with path.open('xb') as out: out.write(b'preexisting NFD destination')
PY_ALIAS
  assert_eq 0 "$?" "NFD destination intruder was created exclusively while apply was stopped"
  kill -CONT "$pid"
  wait "$pid"; code=$?
  assert_eq 3 "$code" "cross-device apply refuses the newly occupied NFC/NFD alias"
  assert_no_panic "cross-device alias refusal" "$(cat "$W/cross.out")"
  assert_eq 'cross-device original' "$(cat "$target")" "cross-device refusal preserves NFC source bytes"
  python3 - "$outer/Documents" <<'PY_PRESERVED'
import pathlib, sys, unicodedata
path = pathlib.Path(sys.argv[1]) / unicodedata.normalize('NFD', 'zz_invoice_café.pdf')
assert path.read_bytes() == b'preexisting NFD destination'
PY_PRESERVED
  assert_eq 0 "$?" "cross-device refusal preserves the NFD destination bytes"
}
cross_device_alias
true
