#!/usr/bin/env bash
# Interruption family: kill during undo.
#
# Undo is itself a mutation — sweep moves files back one at a time. This
# checks two separate things:
#
#   1. Resumption: kill -9 partway through `sweep undo`, then run `sweep undo`
#      again. Does the second run correctly pick up only the files still at
#      their destination, or does it double-restore / strand something?
#
#   2. Convergence: after a killed-then-resumed undo finishes, does the tool
#      ever say "Nothing to undo. This journal was already restored." again —
#      or does every future `sweep undo` call keep re-reporting the same
#      stale progress forever, because the killed run never got to persist
#      what it actually did?
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

make_tree() {
  local d="$1" n="$2"
  mkdir -p "$d"
  for i in $(seq 1 "$n"); do
    : > "$d/Screenshot 2026-0$((i % 9 + 1))-$(printf %02d $((i % 28 + 1))) at $(printf %02d $((i % 12 + 1))).$(printf %02d $((i % 60))).$(printf %02d $((i % 60))) AM ($i).png"
  done
}

# --- Resumption: several kill points across the undo pass -------------------
RESUME_TOTAL=0
RESUME_BAD=0
RESUME_FIRST=""

for target_pct in 5 25 50 75 95; do
  RESUME_TOTAL=$((RESUME_TOTAL + 1))
  d=$(workdir)/D
  N=500
  make_tree "$d" "$N"
  before_set=$(find "$d" -maxdepth 1 -type f -exec basename {} \; | sort)
  before_n=$(echo "$before_set" | grep -c .)

  "$SWEEP" apply "$d" --yes >/dev/null 2>&1
  applied_n=$(find "$d/Screenshots" -type f 2>/dev/null | wc -l | tr -d ' ')

  target=$(( applied_n * target_pct / 100 ))
  [ "$target" -lt 1 ] && target=1

  "$SWEEP" undo >/dev/null 2>&1 &
  pid=$!
  while true; do
    restored=$(find "$d" -maxdepth 1 -type f 2>/dev/null | wc -l | tr -d ' ')
    [ "$restored" -ge "$target" ] && break
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.001
  done
  kill -9 "$pid" 2>/dev/null
  wait "$pid" 2>/dev/null

  mid_origin=$(find "$d" -maxdepth 1 -type f 2>/dev/null | wc -l | tr -d ' ')
  mid_total=$(find "$d" -type f | wc -l | tr -d ' ')

  # Resume: run undo again. It must finish the job without erroring.
  resume_out=$("$SWEEP" undo 2>&1)
  resume_code=$?

  after_set=$(find "$d" -maxdepth 1 -type f -exec basename {} \; | sort)
  after_n=$(find "$d" -type f | wc -l | tr -d ' ')

  bad=""
  if [ "$after_set" != "$before_set" ] || [ "$after_n" != "$before_n" ]; then
    bad="tree wrong after resume: baseline=$before_n post-resume-total=$after_n"
    dupes=$(comm -12 <(find "$d" -maxdepth 1 -type f -exec basename {} \; | sort) <(find "$d" -mindepth 2 -type f -exec basename {} \; | sort))
    [ -n "$dupes" ] && bad="$bad
  duplicated at both origin and destination: $dupes"
    missing=$(comm -23 <(echo "$before_set") <(echo "$after_set"))
    [ -n "$missing" ] && bad="$bad
  missing from origin: $missing"
  fi

  if [ -n "$bad" ]; then
    RESUME_BAD=$((RESUME_BAD + 1))
    [ -z "$RESUME_FIRST" ] && RESUME_FIRST="[kill at ${target_pct}% of undo, mid-state origin=$mid_origin/$before_n total=$mid_total] $bad
  resume said (exit $resume_code): $resume_out"
  fi

  rm -rf "$(dirname "$d")"
done

if [ "$RESUME_BAD" -eq 0 ]; then
  pass "kill -9 during undo at 5/25/50/75/95% progress, then re-run undo: every trial converged back to the exact baseline name set, no double-restore, no stranding"
else
  fail "kill -9 during undo: $RESUME_BAD/$RESUME_TOTAL resume attempts left the tree wrong. First reproduction:
$RESUME_FIRST"
fi

# --- Convergence: does a killed-then-resumed journal ever go quiet again? --
d=$(workdir)/D
N=700
make_tree "$d" "$N"
"$SWEEP" apply "$d" --yes >/dev/null 2>&1
applied_n=$(find "$d/Screenshots" -type f 2>/dev/null | wc -l | tr -d ' ')
target=$(( applied_n / 3 ))

"$SWEEP" undo >/dev/null 2>&1 &
pid=$!
while true; do
  restored=$(find "$d" -maxdepth 1 -type f 2>/dev/null | wc -l | tr -d ' ')
  [ "$restored" -ge "$target" ] && break
  kill -0 "$pid" 2>/dev/null || break
  sleep 0.001
done
kill -9 "$pid" 2>/dev/null
wait "$pid" 2>/dev/null

# Resume to completion (drive it until the tool itself says there's no more
# progress to make, capped so a real bug can't hang the suite).
attempt=0
last_out=""
while [ "$attempt" -lt 5 ]; do
  last_out=$("$SWEEP" undo 2>&1)
  attempt=$((attempt + 1))
  echo "$last_out" | grep -q "Restored 0 files" && break
done

# Now that resuming is done (or gave up trying to make further progress),
# does the journal report a clean "already restored" on the next call — the
# state a fully-reversed journal is supposed to reach?
final_out=$("$SWEEP" undo 2>&1)
final_code=$?

if echo "$final_out" | grep -q "already restored"; then
  pass "after a killed-then-resumed undo finishes, the journal converges: a further \`sweep undo\` correctly reports nothing left to do"
else
  fail "after a killed-then-resumed undo finishes, the journal never converges to 'already restored' — every future \`sweep undo\` call (exit $final_code) keeps re-walking and re-reporting stale progress on this journal, because the killed run never persisted what it actually did before dying. Output on this call:
$final_out"
fi

rm -rf "$(dirname "$d")"
