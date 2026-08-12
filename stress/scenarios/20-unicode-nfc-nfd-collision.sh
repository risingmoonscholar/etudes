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

W=$(workdir); trap 'rm -rf "$W"' EXIT
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

# Both land in one group by their shared ASCII "invoice" token. This part
# is correct and expected: they are genuinely two different directory
# entries (different subdirectories), so scanning both is right.
group_count=$("$SWEEP" "$D" --depth 2 --json | python3 -c "
import json, sys
d = json.load(sys.stdin)
g = [g for g in d['groups'] if g['name'] == 'invoice']
print(g[0]['count'] if g else 0)
")
assert_eq 5 "$group_count" "both NFC and NFD entries were scanned and grouped (not double-counted, not silently merged)"

# The moment that matters: apply. sweep's own contract for two members of an
# accepted group whose destination basenames collide is a clean pre-flight
# refusal: DestinationCollision, exit 2, nothing moved. That is exactly
# what happens for a pure ASCII-case collision (see the sibling scenario).
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

# Nuance: on this filesystem the failure mode is loud (hard_link's EEXIST
# check fires before anything is overwritten), so no byte content was
# actually destroyed. Confirm that explicitly rather than assume it.
survivor_content=""
for f in "$D"/ClientA/*.pdf "$D"/ClientB/*.pdf "$D"/invoice/*.pdf; do
  [ -f "$f" ] || continue
  survivor_content="$survivor_content$(cat "$f")"$'\n'
done
if [[ "$survivor_content" == *"CLIENT-A"* ]] && [[ "$survivor_content" == *"CLIENT-B"* ]]; then
  pass "no byte content was destroyed. both originals are recoverable somewhere on disk"
else
  fail "DATA LOSS: only one of the two original file contents can be found after apply. A=$a_survived B=$b_survived, surviving text: $survivor_content"
fi
unproven "silent overwrite via the cross-device fs::copy fallback" \
  "reproducing this needs a source entry mounted on a second device colliding with a same-device entry; tried with a real hdiutil-attached APFS volume and could not force it. link()'s EEXIST check appears to win over EXDEV on this host before fs::copy is ever reached. that ordering is not something this test can prove holds on every device/filesystem combination"

# Clean up: prove the "resumable" claim in the error text actually is.
"$SWEEP" undo >/dev/null 2>&1
AFTER=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq "$BEFORE" "$AFTER" "undo restores the pre-apply file count despite the partial apply"
