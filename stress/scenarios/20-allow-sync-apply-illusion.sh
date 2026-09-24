#!/usr/bin/env bash
# --allow-sync is documented as the way to proceed inside a cloud-synced
# folder. `sweep PATH --allow-sync` does exactly that: it plans, and even
# prints "warning: this folder is inside a cloud-synced tree". This reads
# as "noted, continuing", not as "this will fail later".
#
# Apply used to ignore this override after a successful plan. This scenario
# keeps the provider-shaped paths but proves the user contract directly:
# successful apply moves the eligible files, and undo returns the exact tree.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT

for provider in "Dropbox" "Google Drive" "OneDrive"; do
  d="$W/$provider/Projects"
  mkdir -p "$d"
  for i in 0 1 2 3 4; do printf '%s-payload-%s\n' "$provider" "$i" > "$d/deck_notes_$i.pdf"; done
  before="$W/${provider// /_}.before.json"
  snapshot_tree "$d" "$before"

  # The flag does what it says at plan time.
  assert_exit 0 "$provider: planning with --allow-sync succeeds" -- "$SWEEP" "$d" --allow-sync

  # The same flag, same root, one command later: apply must honour the plan.
  out=$("$SWEEP" apply "$d" --allow-sync --yes 2>&1)
  code=$?
  if [ "$code" = "0" ]; then
    pass "$provider: apply --allow-sync actually applies, as the plan step implied it would"
  else
    fail "$provider: apply --allow-sync --yes exited $code (wanted 0): ${out%%$'\n'*}"
  fi
  for i in 0 1 2 3 4; do
    [ ! -e "$d/deck_notes_$i.pdf" ] && [ "$(cat "$d/Documents/deck_notes_$i.pdf" 2>/dev/null)" = "$provider-payload-$i" ] \
      && pass "$provider: deck_notes_$i.pdf moved to Documents with its original bytes" \
      || fail "$provider: deck_notes_$i.pdf did not reach Documents exactly"
  done
  assert_exit 0 "$provider: undo restores its allowed sync-folder apply" -- "$SWEEP" undo "$d"
  after="$W/${provider// /_}.after.json"
  snapshot_tree "$d" "$after"
  assert_snapshot_eq "$before" "$after" "$provider: undo restored the exact original tree"
done
