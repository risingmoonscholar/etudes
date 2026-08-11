#!/usr/bin/env bash
# XDG_STATE_HOME points somewhere the tool cannot create a directory. The
# tool must refuse to proceed rather than applying without a journal --
# silently losing undo is exactly the failure this tool must not have.
#
# This overrides the harness's own XDG_STATE_HOME (which lib.sh sets to a
# fresh writable tempdir per scenario) via XDG_STATE_HOME_OVERRIDE, which
# lib.sh reads before making its choice. The read-only parent directory is
# restored to writable in the trap so the harness's own cleanup can still
# remove it.
W_PARENT=$(mktemp -d "${TMPDIR:-/tmp}/etudes-stress-statedir-parent-XXXXXX")
export XDG_STATE_HOME_OVERRIDE="$W_PARENT/state"
chmod 0500 "$W_PARENT"

source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

# Chains onto lib.sh's own EXIT trap (which removes $XDG_STATE_HOME) rather
# than replacing it -- that directory was never actually created here (the
# whole point is that it can't be), but the parent must still be restored to
# writable so removal doesn't fail.
cleanup_extra() { chmod 0700 "$W_PARENT" 2>/dev/null; rm -rf "$XDG_STATE_HOME" "$W_PARENT" 2>/dev/null; }
trap 'cleanup_extra' EXIT

if [ ! -d "$W_PARENT" ] || [ -w "$W_PARENT" ]; then
  unproven "unwritable state dir: apply refuses rather than silently dropping the journal" \
    "could not make a directory unwritable to this uid on this host (e.g. running as root)"
  exit 0
fi

D=$(workdir)
for f in alpha beta gamma delta epsilon; do : > "$D/widget_$f.txt"; done
BEFORE=$(find "$D" -type f | wc -l | tr -d ' ')

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

AFTER=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq "$BEFORE" "$AFTER" "unwritable state dir: no file vanished"

exit 0
