#!/usr/bin/env bash
# ETUDE_STATE_DIR points somewhere the tool cannot create a directory. The
# tool must refuse to proceed rather than applying without a journal --
# silently losing undo is exactly the failure this tool must not have.
#
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

# This must be after source: the direct-run wrapper re-execs the scenario at
# source time, so pre-source filesystem setup would leak from the outer shell.
W_PARENT=$(mktemp -d "${TMPDIR:-/tmp}/etudes-stress-statedir-parent-XXXXXX")
export ETUDE_STATE_DIR_OVERRIDE="$W_PARENT/state"
export ETUDE_STATE_DIR="$ETUDE_STATE_DIR_OVERRIDE"
chmod 0500 "$W_PARENT"

# Restore permissions before removing the unwritable parent. Also remove
# the original harness state directory, bound before the override changed.
cleanup_extra() { chmod 0700 "$W_PARENT" 2>/dev/null; rm -rf "$ETUDE_STATE_DIR" "$W_PARENT" 2>/dev/null; }
trap "cleanup_extra; $_stress_state_cleanup" EXIT

if [ ! -d "$W_PARENT" ] || [ -w "$W_PARENT" ]; then
  unproven "unwritable state dir: apply refuses rather than silently dropping the journal" \
    "could not make a directory unwritable to this uid on this host (e.g. running as root)"
  exit 0
fi

D=$(workdir)
for f in alpha beta gamma delta epsilon; do : > "$D/widget_$f.txt"; done
BEFORE="${D}.before.json"
AFTER="${D}.after.json"
snapshot_tree "$D" "$BEFORE"

# plan touches no state at all -- must be entirely unaffected.
assert_exit 0 "unwritable state dir: plan is unaffected (it never touches state)" -- "$SWEEP" "$D"

APPLY_OUT=$("$SWEEP" apply "$D" --yes 2>&1)
APPLY_CODE=$?

if [ "$APPLY_CODE" = 0 ]; then
  fail "unwritable state dir: apply reported success (exit 0) while unable to write a journal"
else
  pass "unwritable state dir: apply did not report success"
fi

if echo "$APPLY_OUT" | grep -qi "moved [1-9]"; then
  fail "unwritable state dir: apply moved files without being able to record a journal -- undo would be silently impossible: $APPLY_OUT"
else
  pass "unwritable state dir: apply moved nothing when it could not record a journal"
fi

snapshot_tree "$D" "$AFTER"
assert_snapshot_eq "$BEFORE" "$AFTER" "unwritable state dir: apply changed no path or byte"

exit 0
