#!/usr/bin/env bash
# --allow-sync is documented as the way to proceed inside a cloud-synced
# folder. `sweep PATH --allow-sync` does exactly that: it plans, and even
# prints "warning: this folder is inside a cloud-synced tree". This reads
# as "noted, continuing", not as "this will fail later".
#
# It does fail later. apply.rs's own destination-sync guard
# (DestinationIsSynced) is unconditional: it never receives allow_sync at
# all, so `sweep apply PATH --allow-sync --yes` refuses on exactly the same
# root that `sweep PATH --allow-sync` just finished planning successfully.
# Since an accepted group's destination is always a direct child of the
# scanned root, this means: the moment a root needs --allow-sync to be
# scanned, applying to it can never succeed, with or without the flag,
# for Dropbox, Google Drive, OneDrive, every provider sweep recognizes.
#
# The flag doesn't unlock the folder. It unlocks a preview of a folder
# whose organisation apply will then always refuse to write.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT

for provider in "Dropbox" "Google Drive" "OneDrive"; do
  d="$W/$provider/Projects"
  mkdir -p "$d"
  for i in 0 1 2 3 4; do : > "$d/deck_notes_$i.pdf"; done

  # The flag does what it says at plan time.
  assert_exit 0 "$provider: planning with --allow-sync succeeds" -- "$SWEEP" "$d" --allow-sync

  # The same flag, same root, one command later: apply refuses anyway.
  out=$("$SWEEP" apply "$d" --allow-sync --yes 2>&1)
  code=$?
  if [ "$code" = "0" ]; then
    pass "$provider: apply --allow-sync actually applies, as the plan step implied it would"
  else
    fail "REAL DEFECT ($provider): apply --allow-sync --yes exited $code (wanted 0) \
with: ${out%%$'\n'*}. apply.rs's DestinationIsSynced check runs unconditionally \
and never sees the allow_sync flag at all. Since a group's destination is always \
root/<group-name>, ANY root that needed --allow-sync to be scanned will ALSO fail \
this check on apply, always, for every provider, with no override. The flag only \
ever grants a preview; the folder can never actually be organised through it. The \
plan step's own text ('warning: this folder is inside a cloud-synced tree') reads \
as permission granted, which makes the later refusal a surprise rather than a \
documented limit."
  fi
  assert_intact "$d" 5 "$provider: nothing moved by the failed apply attempt"
done
