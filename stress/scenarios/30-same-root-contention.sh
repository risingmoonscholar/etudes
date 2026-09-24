#!/usr/bin/env bash
# Two applies against the SAME root must never turn a filesystem race into
# data loss. One may win and the other may fail, but the loser must leave an
# honest, recoverable state rather than claiming a clean completed apply.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
D="$W/Desktop"; mkdir -p "$D"
for i in $(seq 1 500); do
  printf 'same-root payload %s\n' "$i" > "$D/Screenshot 2026-01-01 at 10.00.00 AM ($i).png"
done

BEFORE="$W/before.json"; AFTER_APPLY="$W/after-apply.json"; AFTER_UNDO="$W/after-undo.json"
snapshot_tree "$D" "$BEFORE"

"$SWEEP" apply "$D" --yes >"$W/first.out" 2>&1 & FIRST=$!
"$SWEEP" apply "$D" --yes >"$W/second.out" 2>&1 & SECOND=$!
wait "$FIRST"; FIRST_STATUS=$?
wait "$SECOND"; SECOND_STATUS=$?

WINS=0
[ "$FIRST_STATUS" -eq 0 ] && WINS=$((WINS + 1))
[ "$SECOND_STATUS" -eq 0 ] && WINS=$((WINS + 1))
assert_eq 1 "$WINS" "same-root contention has exactly one successful apply"

LOSER_OUT="$W/first.out"
[ "$FIRST_STATUS" -eq 0 ] && LOSER_OUT="$W/second.out"
[ "$FIRST_STATUS" -ne 0 ] || [ "$SECOND_STATUS" -ne 0 ] \
  && pass "same-root contender reports a non-success result" \
  || fail "both same-root applies claimed success"
grep -qiE 'could not move|resumable|conflict|already exists' "$LOSER_OUT" \
  && pass "same-root loser explains that its partial state is recoverable" \
  || fail "same-root loser gave no recoverability explanation: $(cat "$LOSER_OUT")"

snapshot_tree "$D" "$AFTER_APPLY"
assert_snapshot_ne "$BEFORE" "$AFTER_APPLY" "the winning apply changed the root layout"
assert_intact "$D" 500 "same-root contention leaves every original payload somewhere in its root"
JOURNALS=$(find "$ETUDE_STATE_DIR" -maxdepth 1 -name 'sweep-*.journal' | wc -l | tr -d ' ')
assert_eq 2 "$JOURNALS" "both same-root attempts retain distinct journal records"

assert_exit 0 "same-root undo resumes and restores the winning apply" -- "$SWEEP" undo "$D"
snapshot_tree "$D" "$AFTER_UNDO"
assert_snapshot_eq "$BEFORE" "$AFTER_UNDO" "same-root recovery restores every original path and byte"
TERMINAL="$W/terminal.json"; snapshot_tree "$D" "$TERMINAL"
assert_exit 1 "same-root second undo reports no remaining work" -- "$SWEEP" undo "$D"
AFTER_TERMINAL="$W/after-terminal.json"; snapshot_tree "$D" "$AFTER_TERMINAL"
assert_snapshot_eq "$TERMINAL" "$AFTER_TERMINAL" "same-root terminal undo changes no path or payload"
