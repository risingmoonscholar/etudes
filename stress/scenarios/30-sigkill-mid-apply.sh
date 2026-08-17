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

# Keep the evidence from a failing trial, because this failure destroys its
# own: the workdir and the journal are both deleted on the way out, so the one
# artifact that would explain a stranding -- the journal the killed apply left
# behind -- is gone at the moment it is produced. Two attempts at diagnosing
# this failed for exactly that reason and had to theorise from the message
# text instead of reading the journal.
#
# Only the FIRST failure in a run is kept. A bad run can strand a dozen files
# across many trials, and 50 copies of a 220-file tree is a disk problem
# rather than evidence.
#
# Nothing here is on the pass path, so a passing run leaves nothing behind.
#
# The "first only" latch is a FILE, not a variable. trial() is called as
# `out=$(trial ...)`, which is a command substitution, which is a subshell:
# a variable set in here never reaches the caller, so every trial would think
# it was the first and a bad run would keep 50 copies. The first version of
# this used a variable and did exactly that -- 12 directories from one run.
keep_evidence() {
  local d="$1" target="$2" pre_undo="$3"
  local DUMP_NOTE=""
  # Inside ETUDE_STATE_DIR, which lib.sh removes when the scenario exits, so
  # the latch cannot outlive the run. A latch in $TMPDIR keyed by pid was the
  # first version: it was never cleaned up, so the files accumulated, and a
  # reused pid would make the NEXT run silently skip keeping evidence -- a
  # quiet failure in the thing built to stop quiet failures.
  #
  # It is a file rather than a variable because trial() is invoked inside a
  # command substitution, which is a subshell, so a variable set there never
  # reaches the caller. That version kept 12 directories from one run.
  #
  # Deleted from the evidence copy below, so it never reads as an artifact.
  local latch="${ETUDE_STATE_DIR:-${TMPDIR:-/tmp}}/.evidence-kept"
  [ -e "$latch" ] && return 0
  : > "$latch" 2>/dev/null

  # Overridable so CI can point it somewhere it can upload from. A runner is
  # destroyed when the job ends, so evidence written to its temp dir is gone
  # before anyone sees it -- which would make keeping it pointless in the one
  # place the failure actually shows up.
  local root="${ETUDES_EVIDENCE_DIR:-${TMPDIR:-/tmp}/etudes-stress-evidence}"
  local dest="$root/${SCENARIO}-$(date +%Y%m%d-%H%M%S)-target$target"
  mkdir -p "$dest" || { echo "  (could not create $dest; evidence not kept)"; return 0; }

  # Both journals: as the kill left it, and as undo left it. The difference
  # between them is often the finding.
  [ -n "$pre_undo" ] && [ -d "$pre_undo" ] && cp -R "$pre_undo" "$dest/state-before-undo" 2>/dev/null
  if [ -n "${ETUDE_STATE_DIR:-}" ] && [ -d "$ETUDE_STATE_DIR" ]; then
    cp -R "$ETUDE_STATE_DIR" "$dest/state-after-undo" 2>/dev/null
    rm -f "$dest/state-after-undo/.evidence-kept"
  fi

  # A copied journal is SEALED with a key from the login keychain, so the file
  # alone is unreadable anywhere but this machine -- and a CI runner is gone by
  # the time anyone opens the artifact. Decrypt now, while the key is still
  # reachable, or the upload preserves ciphertext and nothing else.
  local dump="$(dirname "$SWEEP")/journal-dump"
  if [ -x "$dump" ]; then
    local j
    for j in "$dest"/state-before-undo/*.journal "$dest"/state-after-undo/*.journal; do
      [ -e "$j" ] || continue
      if "$dump" "$j" > "${j%.journal}.plaintext.txt" 2>"${j%.journal}.plaintext.err"; then
        rm -f "${j%.journal}.plaintext.err"
      else
        # Leave the .err, drop the empty .txt. An empty plaintext file beside
        # a journal reads as "the journal was empty", which is a different and
        # wrong conclusion from "it could not be opened".
        rm -f "${j%.journal}.plaintext.txt"
      fi
    done
  else
    DUMP_NOTE="journal-dump was not built, so the kept journals are still sealed and cannot be read off this host"
  fi

  # Names and sizes of the tree, not the files: 220 empty files prove nothing
  # that a listing does not, and a copy invites someone to think the contents
  # mattered. They are all empty by construction.
  find "$d" -type f -exec ls -ld {} \; > "$dest/tree.txt" 2>/dev/null

  # What undo actually said, verbatim, alongside the journal it said it about.
  cp "/tmp/sigkill_trial_undo_out.$$" "$dest/undo-output.txt" 2>/dev/null

  {
    echo "scenario: $SCENARIO"
    echo "kill target: $target"
    echo "sweep binary: $SWEEP"
    [ -n "${DUMP_NOTE:-}" ] && echo "note: $DUMP_NOTE"
    # Resolved from this script, not from $SWEEP: the binary can live
    # anywhere (BIN is overridable), but the scenario is always in the repo.
    echo "commit: $(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  } > "$dest/context.txt" 2>/dev/null

  echo "  evidence kept: $dest"
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
      echo "FAIL target=$target the kill step produced no outcome (got '"'"'$outcome'"'"'). run_and_kill died, so this trial never applied anything and proves nothing"
      rm -f "/tmp/sigkill_trial_undo_out.$$"
      rm -rf "$(dirname "$d")" "$pre_undo"
      return 1
      ;;
  esac

  # Note whether the raw kill (before undo gets a chance to reconcile
  # anything) already produced a duplicate, diagnostic, not itself a fail:
  # the brief's property is checked after undo runs, below.
  local after_kill_n; after_kill_n=$(find "$d" -type f | wc -l | tr -d ' ')
  local pre_undo_note=""
  [ "$after_kill_n" != "$before_n" ] && pre_undo_note=" (tree already had $after_kill_n files, not $before_n, right after the kill, before undo ran at all)"

  # Copy the journal BEFORE undo, because undo rewrites it: it records each
  # reversal as it goes and saves the whole journal at the end. Keeping it
  # afterwards preserves undo's own edits, not the state the killed apply
  # left behind -- which is the state that explains a stranding.
  local pre_undo; pre_undo=$(mktemp -d)
  [ -n "${ETUDE_STATE_DIR:-}" ] && [ -d "$ETUDE_STATE_DIR" ] && cp -R "$ETUDE_STATE_DIR"/. "$pre_undo/" 2>/dev/null

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
    keep_evidence "$d" "$target" "$pre_undo"
    rm -f "/tmp/sigkill_trial_undo_out.$$"
    rm -rf "$(dirname "$d")" "$pre_undo"
    return 1
  fi
  rm -f "/tmp/sigkill_trial_undo_out.$$"
  rm -rf "$(dirname "$d")" "$pre_undo"
  # A positive token, not silence. See run_bucket for why.
  echo "TRIAL-OK"
  return 0
}

# --- Sweep many kill points: very early, very late, and scattered in between.
FIRST_FAILURE=""
TOTAL=0
BAD=0
GOOD=0

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
    if [ "$rc" -eq 0 ] && [ "$out" = "TRIAL-OK" ]; then
      GOOD=$((GOOD + 1))
      continue
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

# The pass requires every trial to have reported success, not merely for none
# of them to have reported failure. Without this, GOOD could be 0 and BAD 0 at
# the same time -- fifty trials that all died silently -- and the line below
# would still call it a pass.
if [ "$GOOD" -ne "$TOTAL" ] && [ "$BAD" -eq 0 ]; then
  fail "SIGKILL mid-apply: only $GOOD of $TOTAL trials reported success and none reported failure. The trials did not run"
elif [ "$BAD" -eq 0 ]; then
  pass "SIGKILL at $TOTAL points across an apply (early/late/scattered): every file was at exactly one place after the kill, and undo returned the full baseline name set every time"
else
  fail "SIGKILL mid-apply: $BAD/$TOTAL trials left the tree wrong after undo (duplicated, lost, or stranded file). First reproduction:
$FIRST_FAILURE"
fi
