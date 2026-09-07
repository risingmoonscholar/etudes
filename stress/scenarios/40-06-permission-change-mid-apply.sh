#!/usr/bin/env bash
# Race: the source directory's permissions change mid-run, after some moves
# already succeeded, in a way that specifically targets the seam inside a
# single move: sweep's same-device path is link(2) the destination into
# existence, THEN unlink(2) the source (apply.rs's own comment: "A crash
# before unlink leaves two readable names for one inode"). link(2) only needs
# read+execute on the source's parent to resolve the name; unlink(2) needs
# write on it. Revoking write but leaving read+execute (chmod 0555) lets the
# link half of a move keep succeeding while the unlink half starts failing.
# This does not touch the destination directory at all.
#
# What must hold: sweep reports the failure honestly (nonzero exit), and the
# journal reflects reality: every entry it claims is "done" actually
# finished, and undo can act on exactly those.
#
# Nothing here is real data. Every tree is generated and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require python3 "builds fixture trees fast and reads the --json plan" || exit 0

N=500

build_tree() {
  mkdir -p "$1"
  python3 - "$1" "$N" <<'PY'
import sys, os
d, n = sys.argv[1], int(sys.argv[2])
def letters(i, width=5):
    s = ""
    for _ in range(width):
        s = chr(97 + i % 26) + s
        i //= 26
    return s
for i in range(n):
    open(os.path.join(d, f"batch_{letters(i)}.csv"), "w").close()
PY
}

now_ms() { python3 -c 'import time; print(int(time.time()*1000))'; }

W=$(workdir)
cleanup() {
  # Undo the permission change first: rm -rf can't touch a 0555 directory's
  # contents.
  [ -n "${D:-}" ] && chmod 755 "$D" 2>/dev/null
  rm -rf "$W" 2>/dev/null
}
trap cleanup EXIT

CTRL="$W/control/Desktop"; build_tree "$CTRL"
T0_START=$(now_ms)
"$SWEEP" apply "$CTRL" --yes >/dev/null 2>&1
T0_END=$(now_ms)
T0=$((T0_END - T0_START))
if [ "$T0" -lt 30 ]; then
  unproven "permission change mid-apply" "baseline apply of $N files finished in ${T0}ms on this host. Too fast for a background process to land inside the window"
  exit 0
fi

D="$W/attempt/Desktop"
build_tree "$D"
BEFORE=$(find "$D" -type f | wc -l | tr -d ' ')

"$SWEEP" apply "$D" --yes >"$W/apply.out" 2>"$W/apply.err" &
PID=$!
DELAY_MS=$((T0 * 45 / 100))
python3 -c "import time; time.sleep($DELAY_MS/1000)"
# r-xr-xr-x: still resolvable (needed for link(2) to find the source by
# name) but no longer writable (needed for unlink(2) to remove the entry).
chmod 555 "$D"
wait "$PID"
CODE=$?
chmod 755 "$D"

if [ "$CODE" = "0" ]; then
  unproven "permission change mid-apply" "apply finished (exit 0) before the chmod at ${DELAY_MS}ms into a ~${T0}ms baseline run landed inside the window. Could not exercise the race on this host"
  exit 0
fi

echo "    (chmod fired at ~${DELAY_MS}ms into a ~${T0}ms baseline run, apply exited $CODE)"

assert_eq 1 "$([ "$CODE" != "0" ] && echo 1 || echo 0)" "apply reported the permission failure rather than silently succeeding (exit $CODE)"

if [ -s "$W/apply.err" ] && grep -qi "sweep:" "$W/apply.err"; then
  pass "the failure was printed, not swallowed"
else
  fail "apply exited $CODE but printed nothing identifiable on stderr: $(cat "$W/apply.err")"
fi

# The signature we're looking for: a name that exists BOTH at its original
# source path and at its destination. link(2) succeeded, unlink(2) failed,
# and nothing rolled the link back. If this exists, no file's content was
# lost (same inode, two names) but the journal's own "done" bookkeeping and
# the tree's real state have diverged.
DUP_NAMES=()
while IFS= read -r destpath; do
  name="$(basename "$destpath")"
  srcpath="$D/$name"
  if [ -f "$srcpath" ]; then
    DUP_NAMES+=("$name")
  fi
done < <(find "$D" -mindepth 2 -type f 2>/dev/null)

if [ "${#DUP_NAMES[@]}" -eq 0 ]; then
  # No duplicate: either nothing was in flight at exactly the link/unlink
  # boundary when chmod landed (bad luck on timing), or unlink somehow still
  # succeeded, or link itself started failing too once the parent lost write
  # (some platforms fold search and write together for path resolution
  # purposes in ways this attack doesn't assume). Report the miss honestly
  # rather than asserting a bug that didn't reproduce this run.
  unproven "permission change mid-apply leaves an untracked duplicate" "no file existed at both its source and destination path after the chmod-induced failure this run. The exact link-succeeds/unlink-fails interleaving did not land"
else
  NAME="${DUP_NAMES[0]}"
  SRC_INODE=$(stat -f '%i' "$D/$NAME")
  DEST_PATH=$(find "$D" -mindepth 2 -name "$NAME" 2>/dev/null | head -1)
  DEST_INODE=$(stat -f '%i' "$DEST_PATH")

  echo "    (found: $NAME exists at both $D/$NAME and $DEST_PATH)"

  assert_eq "$SRC_INODE" "$DEST_INODE" "the duplicate is the SAME inode (link succeeded, no content was copied or lost). Just two names now"

  fail "REAL DEFECT: a permission failure between link(2) and unlink(2) inside move_one leaves a file with two live names (the original at \$D/$NAME and a hard-linked copy at $DEST_PATH), and apply.rs never marks that journal entry done (the \`done = true\` assignment sits after move_one returns Ok, so a mid-move_one failure skips it). \`sweep undo\` will therefore never offer to clean up the duplicate: it only reverses entries the journal marked done, and this one is not. The duplicate is permanent and untracked until removed by hand."

  # Confirm the prediction directly: undo must not touch this duplicate.
  UNDO_OUT=$("$SWEEP" undo 2>&1)
  if [ -f "$D/$NAME" ] && [ -f "$DEST_PATH" ]; then
    fail "confirmed: after 'sweep undo', the duplicate at $DEST_PATH is still there and still untracked. undo output: $UNDO_OUT"
  else
    pass "undo actually cleaned up the duplicate after all (better than expected from reading the source)"
  fi
fi

# Whatever else happened, no file's CONTENT should have been destroyed:
# every name still resolves to zero-byte content (all fixtures are empty),
# and nothing should be missing entirely (only possibly duplicated).
MISSING=0
while IFS= read -r name; do
  if [ ! -e "$D/$name" ] && ! find "$D" -mindepth 2 -name "$name" 2>/dev/null | grep -q .; then
    MISSING=$((MISSING + 1))
  fi
done < <(python3 -c "
def letters(i, width=5):
    s = ''
    for _ in range(width):
        s = chr(97 + i % 26) + s
        i //= 26
    return s
for i in range($N):
    print(f'batch_{letters(i)}.csv')
")
assert_eq 0 "$MISSING" "no file went missing outright (duplication is possible, disappearance is not)"
