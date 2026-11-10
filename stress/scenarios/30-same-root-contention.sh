#!/usr/bin/env bash
# Two applies against the SAME root must never turn a filesystem race into
# data loss. One may win and the other may fail, but the loser must leave an
# honest, recoverable state rather than claiming a clean completed apply.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir) || exit 1; trap 'rm -rf "$W"' EXIT
D="$W/Desktop"; mkdir -p "$D"
for i in $(seq 1 500); do
  printf 'same-root payload %s\n' "$i" > "$D/Screenshot 2026-01-01 at 10.00.00 AM ($i).png"
done

BEFORE="$W/before.json"; AFTER_APPLY="$W/after-apply.json"; AFTER_UNDO="$W/after-undo.json"
snapshot_tree "$D" "$BEFORE"

# Freeze the first operation after journalled progress, keep it stopped for
# the second operation's entire run, then resume it against the changed root.
contend() {
  local root="$1" state="$2" output="$3" target first
  target=$("$SWEEP" "$root" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["details"]["groups"][0]["members"][-1])')
  ETUDE_STATE_DIR="$state" "$SWEEP" apply "$root" --yes >"$output/first.out" 2>&1 & FIRST=$!
  if ! "$(dirname "$SWEEP")/journal-dump" --watch-apply "$state" "$root" "$target" "$FIRST" >"$output/witness" 2>"$output/watch.err"; then
    kill -CONT "$FIRST" 2>/dev/null || true
    wait "$FIRST"; FIRST_STATUS=$?
    assert_eq 0 "$FIRST_STATUS" "unwitnessed first apply still succeeds"
    unproven "same-root overlap" "could not stop first apply with a pending entry: $(cat "$output/watch.err")"
    return 1
  fi
  if [[ "$(ps -o state= -p "$FIRST")" != *T* ]] || [ ! -f "$target" ]; then
    fail "same-root watcher did not preserve a stopped pending entry"
    kill -CONT "$FIRST" 2>/dev/null || true
    wait "$FIRST" 2>/dev/null || true
    return 1
  fi
  pass "same-root first apply is stopped with a pending file"
  ETUDE_STATE_DIR="$state" "$SWEEP" apply "$root" --yes >"$output/second.out" 2>&1
  SECOND_STATUS=$?
  [[ "$(ps -o state= -p "$FIRST")" == *T* ]] \
    && pass "same-root second apply finished while first remained stopped" || fail "same-root overlap witness was lost"
  kill -CONT "$FIRST"
  wait "$FIRST"; FIRST_STATUS=$?
}
contend "$D" "$ETUDE_STATE_DIR" "$W" || exit 0

WINS=0
[ "$FIRST_STATUS" -eq 0 ] && WINS=$((WINS + 1))
[ "$SECOND_STATUS" -eq 0 ] && WINS=$((WINS + 1))
assert_eq 1 "$WINS" "same-root contention has exactly one successful apply"

LOSER_OUT="$W/first.out"
[ "$FIRST_STATUS" -eq 0 ] && LOSER_OUT="$W/second.out"
[ "$FIRST_STATUS" -ne 0 ] || [ "$SECOND_STATUS" -ne 0 ] \
  && pass "same-root contender reports a non-success result" \
  || fail "both same-root applies claimed success"
grep -qiE 'could not move|changed just now|already exists|continue from where' "$LOSER_OUT" \
  && pass "same-root loser explains that its partial state is recoverable" \
  || fail "same-root loser gave no recoverability explanation: $(cat "$LOSER_OUT")"

snapshot_tree "$D" "$AFTER_APPLY"
assert_snapshot_ne "$BEFORE" "$AFTER_APPLY" "the winning apply changed the root layout"
assert_intact "$D" 500 "same-root contention leaves every original payload somewhere in its root"
JOURNALS=$(find "$ETUDE_STATE_DIR" -maxdepth 1 -name 'sweep-*.journal' | wc -l | tr -d ' ')
assert_eq 2 "$JOURNALS" "both same-root attempts retain distinct journal records"

# Each apply owns its moved share, including the partially completed loser.
# Drain both operations and bind their printed counters to the actual tree.
recover_operations() {
  local root="$1" state="$2" output="$3" total="$4" before="$5"
  local accounting moved operations restored=0 successful=0 step out code n top_before top_after
  accounting=$(python3 - "$output/first.out" "$output/second.out" <<'PY_MOVED'
import pathlib, re, sys
counts = []
for name in sys.argv[1:]:
    text = pathlib.Path(name).read_text()
    matches = re.findall(r'^Moved ([0-9]+) files?\.$', text, re.M)
    matches += re.findall(r'^([0-9]+) (?:file was|files were) moved before this stopped\.', text, re.M)
    if not matches and 'Nothing was moved, so there is nothing to undo.' in text:
        matches = ['0']
    assert len(matches) == 1, f'{name}: expected one moved count, got {matches}'
    counts.append(int(matches[0]))
assert any(n > 0 for n in counts), f'at least one operation must move files: {counts}'
print(sum(counts), sum(n > 0 for n in counts))
PY_MOVED
  )
  read -r moved operations <<< "$accounting"
  case "$operations" in 1|2) ;; *) fail "could not determine the number of nonempty apply operations: $accounting"; return;; esac
  assert_eq "$total" "$moved" "same-root operations report every moved payload exactly once"
  assert_eq "$moved" "$(find "$root" -mindepth 2 -type f | wc -l | tr -d ' ')" "same-root reported moves equal files at destinations"
  # Decide the exact undo count from apply, before observing any undo result.
  for ((step=1; step<=operations; step++)); do
    top_before=$(find "$root" -maxdepth 1 -type f | wc -l | tr -d ' ')
    out=$(ETUDE_STATE_DIR="$state" "$SWEEP" undo "$root" 2>&1); code=$?
    printf '%s\n' "$out" > "$output/undo-$step.out"
    assert_eq 0 "$code" "same-root undo $step restores an operation successfully"
    [ "$code" = 0 ] || break
    successful=$((successful + 1))
    n=$(python3 - "$output/undo-$step.out" <<'PY_RESTORED'
import pathlib, re, sys
counts = re.findall(r'^Restored ([0-9]+) files?\.$', pathlib.Path(sys.argv[1]).read_text(), re.M)
assert len(counts) == 1 and int(counts[0]) > 0, f'expected a positive restored count: {counts}'
print(counts[0])
PY_RESTORED
    )
    case "$n" in ''|*[!0-9]*) fail "same-root undo $step omitted its restored count: $out"; break;; esac
    top_after=$(find "$root" -maxdepth 1 -type f | wc -l | tr -d ' ')
    assert_eq "$n" "$((top_after - top_before))" "same-root undo $step restored count matches files returned home"
    restored=$((restored + n))
  done
  assert_eq "$operations" "$successful" "same-root successful undo count equals the nonempty apply count"
  assert_eq "$moved" "$restored" "same-root undo counts sum to both operations' moved counts"
  snapshot_tree "$root" "$output/after-undo.json"
  assert_snapshot_eq "$before" "$output/after-undo.json" "same-root recovery restores every original path and byte"
  snapshot_tree "$root" "$output/exhaustion-before.json"
  out=$(ETUDE_STATE_DIR="$state" "$SWEEP" undo "$root" 2>&1); code=$?
  assert_eq 1 "$code" "same-root undo after exactly $operations operations reports no remaining work"
  grep -qi 'nothing to undo' <<<"$out" \
    && pass "same-root exact undo count exhausts both operation records" \
    || fail "same-root undo hid exhaustion: $out"
  snapshot_tree "$root" "$output/exhaustion-after.json"
  assert_snapshot_eq "$output/exhaustion-before.json" "$output/exhaustion-after.json" "same-root exhaustion changes no path or byte"
  snapshot_tree "$root" "$output/terminal.json"
  out=$(ETUDE_STATE_DIR="$state" "$SWEEP" undo "$root" 2>&1); code=$?
  assert_eq 1 "$code" "same-root extra terminal undo reports no remaining work"
  grep -qi 'nothing to undo' <<<"$out" \
    && pass "same-root extra terminal undo explains there is nothing to undo" \
    || fail "same-root extra terminal undo hid exhaustion: $out"
  snapshot_tree "$root" "$output/after-terminal.json"
  assert_snapshot_eq "$output/terminal.json" "$output/after-terminal.json" "same-root terminal undo changes no path or payload"
}
recover_operations "$D" "$ETUDE_STATE_DIR" "$W" 500 "$BEFORE"

# One overlap can miss the short window between one attempt's plan check and
# its journal. Repeat with fresh folders, so that a loser beaten in that window
# is seen: it must still explain itself and keep its own record.
for round in $(seq 1 15); do
  R="$W/round-$round"; mkdir -p "$R/Desktop" "$R/state"
  for i in $(seq 1 200); do
    printf 'same-root payload %s\n' "$i" > "$R/Desktop/Screenshot 2026-01-01 at 10.00.00 AM ($i).png"
  done
  snapshot_tree "$R/Desktop" "$R/before.json"
  contend "$R/Desktop" "$R/state" "$R" || continue
  LOSER_OUT="$R/first.out"
  [ "$FIRST_STATUS" -eq 0 ] && LOSER_OUT="$R/second.out"
  grep -qiE 'could not move|changed just now|already exists|continue from where' "$LOSER_OUT" \
    && pass "round $round: the loser explains itself" \
    || fail "round $round: the loser gave no explanation: $(cat "$LOSER_OUT")"
  JOURNALS=$(find "$R/state" -maxdepth 1 -name 'sweep-*.journal' | wc -l | tr -d ' ')
  assert_eq 2 "$JOURNALS" "round $round: both attempts keep a journal record"
  assert_intact "$R/Desktop" 200 "round $round: every payload is still in the folder"
  recover_operations "$R/Desktop" "$R/state" "$R" 200 "$R/before.json"
done
