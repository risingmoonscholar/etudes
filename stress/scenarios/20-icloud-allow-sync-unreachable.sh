#!/usr/bin/env bash
# The most common cloud-synced folder on a real Mac is iCloud Drive, and on
# a Mac with "Desktop & Documents" sync turned on (a very common default),
# the user's actual Desktop lives at:
#
#   ~/Library/Mobile Documents/com~apple~CloudDocs/Desktop
#
# sweep advertises one escape hatch for "this folder is cloud-synced":
# --allow-sync. It works for Dropbox and Google Drive (checked below as a
# control). This scenario checks whether it also works for the sync
# provider it will actually meet most often on this OS.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT

# Build the exact real macOS path shape for each provider, all under our own
# scratch tree so nothing touches a real home directory.
ICLOUD="$W/Library/Mobile Documents/com~apple~CloudDocs/Desktop"
DROPBOX="$W/Dropbox/Projects"
GDRIVE="$W/Google Drive/My Drive"
mkdir -p "$ICLOUD" "$DROPBOX" "$GDRIVE"

for d in "$ICLOUD" "$DROPBOX" "$GDRIVE"; do
  for i in 0 1 2 3 4; do : > "$d/deck_notes_$i.pdf"; done
done

# --- Baseline: all three are correctly detected and refused by default ---
assert_exit 2 "iCloud Desktop is refused by default (no --allow-sync)"    -- "$SWEEP" "$ICLOUD"
assert_exit 2 "Dropbox is refused by default (no --allow-sync)"           -- "$SWEEP" "$DROPBOX"
assert_exit 2 "Google Drive is refused by default (no --allow-sync)"      -- "$SWEEP" "$GDRIVE"

# --- The escape hatch: works for the other two providers ---
assert_exit 0 "Dropbox proceeds with --allow-sync"      -- "$SWEEP" "$DROPBOX" --allow-sync
assert_exit 0 "Google Drive proceeds with --allow-sync" -- "$SWEEP" "$GDRIVE" --allow-sync

# --- The actual claim under test ---
icloud_out=$("$SWEEP" "$ICLOUD" --allow-sync 2>&1)
icloud_code=$?
if [ "$icloud_code" = "0" ]; then
  pass "iCloud Desktop proceeds with --allow-sync, same as Dropbox and Google Drive"
else
  fail "REAL DEFECT: --allow-sync cannot unlock iCloud Drive at all. \
sweep '$ICLOUD' --allow-sync exited $icloud_code (wanted 0) with: ${icloud_out%%$'\n'*} \
— iCloud Drive always contains a path component literally named 'Library', \
which scan.rs's NEVER_ENTER system-location check refuses UNCONDITIONALLY, \
before is_synced()/allow_sync is ever consulted. The sync-specific refusal \
and its documented override never get a chance to run for the one sync \
provider built into every Mac. A user with iCloud Desktop sync on \
(a common default) cannot run sweep on their own Desktop, with any flag."
fi

# Confirm the failure text is specifically the system-location refusal, not
# the sync refusal — proof the two code paths never actually meet for this
# input, not just a wording coincidence.
if [ "$icloud_code" != "0" ]; then
  case "$icloud_out" in
    *"system or credential location"*)
      pass "diagnostic: confirmed as RefusedSystemLocation, not RefusedSyncRoot — allow_sync structurally cannot reach this branch" ;;
    *)
      fail "diagnostic: refusal text changed shape ('$icloud_out') — re-check whether this is still the same root cause" ;;
  esac
fi

# Whatever happened above, nothing should have actually moved anything —
# these were all plan-only invocations.
assert_intact "$ICLOUD" 5 "iCloud Desktop untouched by plan-only runs"
assert_intact "$DROPBOX" 5 "Dropbox untouched by plan-only runs"
assert_intact "$GDRIVE" 5 "Google Drive untouched by plan-only runs"
