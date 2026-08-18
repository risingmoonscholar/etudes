#!/usr/bin/env bash
# Interruption family: SIGKILL mid-apply.
#
# sweep's whole undo promise rests on one claim: a crash mid-move leaves every
# file at its origin or its destination, never lost, never duplicated. This
# attacks that claim directly by kill -9'ing a real apply at many different
# points (including as early and as late as the run allows) and checking
# the tree by NAME SET, not just by count, both right after the kill and
# again after `sweep undo`.
#
# No timeout/gtimeout is used: this host has neither. Interruption is timed
# by polling the destination directory for a specific number of moved files,
# which is exact regardless of host speed, then delivering the signal
# immediately. This is more reliable than a wall-clock sleep would be.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

# One screenshot-shaped tree of N files, named so every filename is unique.
make_tree() {
  local d="$1" n="$2"
  mkdir -p "$d"
  for i in $(seq 1 "$n"); do
    : > "$d/Screenshot 2026-0$((i % 9 + 1))-$(printf %02d $((i % 28 + 1))) at $(printf %02d $((i % 12 + 1))).$(printf %02d $((i % 60))).$(printf %02d $((i % 60))) AM ($i).png"
  done
}

# Run one apply, kill -9 once the destination has TARGET moved files (or the
# process exits first, meaning the whole apply beat the target, too fast to
# use for this trial). Echoes: "killed" or "finished-early".
run_and_kill() {
  local d="$1" target="$2"
  "$SWEEP" apply "$d" --yes >/dev/null 2>&1 &
  local pid=$!
  while true; do
    local moved
    moved=$(find "$d/Screenshots" -type f 2>/dev/null | wc -l | tr -d ' ')
    [ "$moved" -ge "$target" ] && break
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "finished-early"
      return
    fi
    sleep 0.001
  done
  kill -9 "$pid" 2>/dev/null
  wait "$pid" 2>/dev/null
  echo "killed"
}

# One full trial: build, apply+kill at TARGET, check the tree is duplicate-
# and loss-free, undo, check the tree is back to exactly the baseline NAME
# SET. Returns 0 if every property held, 1 and prints why otherwise.
trial() {
  local target="$1" n="$2"
  local d; d=$(workdir)/D
  make_tree "$d" "$n"
  local before_set; before_set=$(find "$d" -maxdepth 1 -type f -exec basename {} \; | sort)
  local before_n; before_n=$(echo "$before_set" | grep -c .)

  local outcome; outcome=$(run_and_kill "$d" "$target")

  # The trial checks its own instrument before trusting the result. run_and_kill
  # runs inside a command substitution, so if it dies -- an unbound variable
  # under set -u, a missing binary -- the death does not propagate here. It just
  # returns nothing. The apply never ran, the tree is untouched, undo has nothing
  # to reverse, before and after match, and the trial PASSES having tested
  # nothing. That is the exact shape of the failure this scenario exists to
  # catch, occurring inside the scenario itself; it happened while this file was
  # being edited and reported ok across all 50 trials.
  case "$outcome" in
    killed|finished-early) ;;
    *)
      echo "FAIL target=$target the kill step produced no outcome (got [$outcome]). run_and_kill died, so this trial never applied anything and proves nothing"
      rm -f "/tmp/sigkill_trial_undo_out.$$"
      rm -rf "$(dirname "$d")"
      return 1
      ;;
  esac

  # Note whether the raw kill (before undo gets a chance to reconcile
  # anything) already produced a duplicate, diagnostic, not itself a fail:
  # the brief's property is checked after undo runs, below.
  local after_kill_n; after_kill_n=$(find "$d" -type f | wc -l | tr -d ' ')
  local pre_undo_note=""
  [ "$after_kill_n" != "$before_n" ] && pre_undo_note=" (tree already had $after_kill_n files, not $before_n, right after the kill, before undo ran at all)"

  "$SWEEP" undo >/tmp/sigkill_trial_undo_out.$$ 2>&1
  local after_set; after_set=$(find "$d" -maxdepth 1 -type f -exec basename {} \; | sort)
  local after_n; after_n=$(find "$d" -type f | wc -l | tr -d ' ')

  if [ "$after_set" != "$before_set" ] || [ "$after_n" != "$before_n" ]; then
    echo "FAIL target=$target outcome=$outcome$pre_undo_note"
    echo "  baseline count=$before_n, post-undo total count=$after_n"
    local dupes; dupes=$(comm -12 <(find "$d" -maxdepth 1 -type f -exec basename {} \; | sort) <(find "$d" -mindepth 2 -type f -exec basename {} \; | sort))
    if [ -n "$dupes" ]; then
      echo "  duplicated (same file present at BOTH origin and its sorted destination, forever, because sweep undo left it there):"
      echo "$dupes" | sed 's/^/    /'
    fi
    local missing; missing=$(comm -23 <(echo "$before_set") <(echo "$after_set"))
    if [ -n "$missing" ]; then
      echo "  missing from origin after undo (not necessarily lost: check if it is stranded elsewhere):"
      echo "$missing" | sed 's/^/    /'
    fi
    echo "  sweep undo said:"
    sed 's/^/    /' "/tmp/sigkill_trial_undo_out.$$"
    rm -f "/tmp/sigkill_trial_undo_out.$$"
    rm -rf "$(dirname "$d")"
    return 1
  fi
  rm -f "/tmp/sigkill_trial_undo_out.$$"
  rm -rf "$(dirname "$d")"
  # A positive token, not silence, and it carries the outcome: a run that
  # never managed to kill anything proves nothing about crash safety, and
  # "finished-early" every time would otherwise pass. See run_bucket.
  echo "TRIAL-OK $outcome"
  return 0
}

# --- Sweep many kill points: very early, very late, and scattered in between.
FIRST_FAILURE=""
TOTAL=0
BAD=0
GOOD=0
KILLED=0

# A trial counts as passed only if it SAYS so and exits 0. It used to count as
# passed by printing nothing, which meant any way of dying quietly read as
# success: an edit to this file once added an unbound variable to a helper, so
# under `set -u` every one of the 50 trials died before asserting anything, and
# the scenario reported ok. Fifty crashed trials, one green line.
#
# run.sh already refuses a scenario that produced no assertions at all. This is
# the same rule one level down, for a scenario that produces its assertion from
# trials that never ran. Silence is not success here either.
run_bucket() {
  local label="$1"; shift
  local targets=("$@")
  for t in "${targets[@]}"; do
    TOTAL=$((TOTAL + 1))
    local out rc
    out=$(trial "$t" 220); rc=$?
    if [ "$rc" -eq 0 ]; then
      case "$out" in
        "TRIAL-OK killed")         GOOD=$((GOOD + 1)); KILLED=$((KILLED + 1)); continue ;;
        "TRIAL-OK finished-early") GOOD=$((GOOD + 1)); continue ;;
      esac
    fi
    BAD=$((BAD + 1))
    # A trial that died without saying why: report the corpse rather than
    # letting an empty message stand in for a diagnosis.
    [ -z "$out" ] && out="the trial produced no output and exited $rc. It died before it could assert anything"
    [ -z "$FIRST_FAILURE" ] && FIRST_FAILURE="[$label target=$t]
$out"
  done
}

# Very early: kill as soon as 1-3 files have landed.
run_bucket "very-early" 1 1 2 2 3 3 1 2 3 1
# Very late: kill with only a handful of files left to move (n=220).
run_bucket "very-late" 214 215 216 217 214 215 216 217 215 216
# Scattered across the middle of the run. Built into a list and handed to
# run_bucket rather than looping here: this block used to carry its own copy
# of the pass/fail check, and when the check changed in one place it did not
# change in the other -- 30 trials passing and being counted as failures.
# One place decides what a passed trial looks like.
SCATTERED=()
for i in $(seq 1 30); do
  SCATTERED+=( $(( (i * 37) % 205 + 5 )) )
done
run_bucket "scattered" "${SCATTERED[@]}"

# Order matters here. A real failure is the most specific thing that can have
# happened, so it is reported first; the checks under it are about whether the
# trials ran at all, and putting one of those first would mask a genuine
# stranding behind "nothing was killed". That is exactly what happened when
# these were written in the other order.
if [ "$BAD" -gt 0 ]; then
  fail "SIGKILL mid-apply: $BAD/$TOTAL trials left the tree wrong after undo (duplicated, lost, or stranded file). First reproduction:
$FIRST_FAILURE"
elif [ "$GOOD" -ne "$TOTAL" ]; then
  # No failures and not enough successes: the trials did not run. Silence is
  # not success, one level down from run.sh's zero-assertion guard.
  fail "SIGKILL mid-apply: only $GOOD of $TOTAL trials reported success and none reported failure. The trials did not run"
elif [ "$KILLED" -eq 0 ]; then
  # Every apply beat its kill target, so nothing was ever interrupted. The tree
  # came back intact because it was never disturbed, which is not evidence
  # about crash safety. On a fast enough host, or with a sweep that exits
  # immediately, this is how the scenario would quietly stop testing anything.
  fail "SIGKILL mid-apply: no trial managed to kill an apply in progress ($TOTAL trials, all finished before the kill point). This proves nothing about interruption"
else
  pass "SIGKILL at $TOTAL points across an apply (early/late/scattered), $KILLED of them killed mid-run: every file was at exactly one place after the kill, and undo returned the full baseline name set every time"
fi
