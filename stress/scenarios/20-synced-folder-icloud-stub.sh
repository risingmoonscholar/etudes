#!/usr/bin/env bash
# Sync-client detection breadth, plus the specific hazard every sync client
# shares: when a file is "evicted" to save local space (iCloud Optimise
# Storage, Dropbox Smart Sync, Google Drive streaming), what's left on disk
# in its place is a placeholder: for iCloud, a zero-byte stub named
# ".<original name>.icloud". Moving that stub anywhere and calling it the
# file would silently hand the user an empty husk instead of their document.
#
# The apply half of this check deliberately runs on a PLAIN (non-synced)
# directory. Applying inside an actually-synced root is proven broken on
# its own terms in 20-allow-sync-apply-illusion.sh. Mixing that defect
# into this scenario would make a stub-handling failure indistinguishable
# from the unrelated apply-refusal defect. This isolates one property at a
# time, which is the same reason sweep's own detectors stay single-purpose.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT

# --- Detection breadth: every documented marker refuses by default ---
for provider in "Dropbox" "OneDrive" "Sync.com" "pCloud Drive"; do
  d="$W/$provider/Projects"
  mkdir -p "$d"
  : > "$d/placeholder.txt"
  assert_exit 2 "$provider is detected as a synced folder and refused by default" -- "$SWEEP" "$d"
done

# --- The .icloud eviction stub, scanned inside an --allow-sync'd root ---
SYNCED="$W/Dropbox/ClientWork"
mkdir -p "$SYNCED"
for i in 0 1 2 3 4; do echo "REAL-CONTENT-$i" > "$SYNCED/deck_notes_$i.pdf"; done
# The 6th "Documents" file has been evicted: its actual bytes are gone from this
# machine, replaced by a zero-byte placeholder Finder shows with a cloud
# icon. The placeholder's name is dot-prefixed and carries the original
# name plus .icloud, per Apple's on-disk convention for this state.
: > "$SYNCED/.deck_notes_5.pdf.icloud"

json=$("$SWEEP" "$SYNCED" --allow-sync --json 2>&1)
scanned=$(python3 -c "import json,sys; print(json.load(sys.stdin)['scanned'])" <<<"$json" 2>/dev/null)
hidden=$(python3 -c "import json,sys; print(json.load(sys.stdin)['skipped']['hidden'])" <<<"$json" 2>/dev/null)
stub_in_group=$(python3 -c "
import json, sys
d = json.load(sys.stdin)
members = [m for g in d['groups'] for m in g['members']]
print(any('.icloud' in m for m in members))
" <<<"$json" 2>/dev/null)

assert_eq 5 "$scanned" "the stub is not counted among the real files"
assert_eq 1 "$hidden" "the stub is skipped as a hidden (dot-prefixed) entry"
assert_eq "False" "$stub_in_group" "the stub never appears as a group member"

# --- Apply, on a plain root with the same fixture shape (isolating this
#     property from the separate, already-proven apply/--allow-sync defect)
PLAIN="$W/LocalDesktop"
mkdir -p "$PLAIN"
for i in 0 1 2 3 4; do echo "REAL-CONTENT-$i" > "$PLAIN/deck_notes_$i.pdf"; done
: > "$PLAIN/.deck_notes_5.pdf.icloud"

assert_exit 0 "apply proceeds on a plain root with the same stub shape" \
  -- "$SWEEP" apply "$PLAIN" --yes

moved_dir=$(find "$PLAIN" -maxdepth 1 -type d -name Documents)
if [ -z "$moved_dir" ]; then
  fail "expected a 'Documents' group directory after apply; none found"
else
  moved_count=$(find "$moved_dir" -type f | wc -l | tr -d ' ')
  assert_eq 5 "$moved_count" "exactly the 5 real files moved. the stub did not tag along"
  stub_survived=0
  [ -f "$PLAIN/.deck_notes_5.pdf.icloud" ] && stub_survived=1
  assert_eq 1 "$stub_survived" "the eviction stub is still exactly where it was, untouched"
  stub_size=$(stat -f%z "$PLAIN/.deck_notes_5.pdf.icloud" 2>/dev/null)
  assert_eq 0 "$stub_size" "the stub is still zero bytes. nothing wrote fake content into it"
fi
