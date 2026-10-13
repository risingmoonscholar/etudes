#!/usr/bin/env bash
# Race: the destination directory sweep is about to create already exists.
# But it is a plain FILE, not a directory. `mkdir -p` (fs::create_dir_all) on a
# path that exists and is not a directory must fail, not silently write into
# something that isn't a directory and not crash.
#
# Two variants:
#   A. deterministic: the blocking file is already there before sweep's
#      single scan+plan+apply invocation starts at all. This is the cleanest,
#      always-reproducible form of the collision and pins down the exact
#      behaviour.
#   B. genuinely mid-run: a large first group (processed before the target
#      group, by plan order) provides journalled progress. The watcher stops
#      apply and proves the second group's target is pending before the blocking
#      file is planted, then apply resumes.
#
# What must hold, in both: sweep refuses cleanly (no crash, no data loss, no
# writing into the file-that-should-have-been-a-directory), reports the
# failure honestly, and whatever it already moved before hitting the
# collision stays moved and correct.
#
# Nothing here is real data. Every tree is generated and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require python3 "builds fixture trees fast" || exit 0

# --- Variant A: deterministic ---------------------------------------------

# Letter-suffixed names so exactly one token repeats across the set ("Documents")
# and none of the per-file unique suffixes accidentally forms a second
# same-sized candidate group that could win the tie-break instead.
letters() {  # letters N -> deterministic short letter code, distinct per N
  python3 -c "
i=$1
s=''
for _ in range(5):
    s = chr(97 + i % 26) + s
    i //= 26
print(s)"
}

CONTROL="${1:-}"
case "$CONTROL" in ''|--control-late|--control-ignored) ;; *) fail "unknown mid-apply control"; exit 0;; esac
DUMP="$(dirname "$SWEEP")/journal-dump"
if [ ! -x "$DUMP" ]; then
  unproven "mid-apply injection" "dev-only journal watcher unavailable"
  exit 0
fi
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
W=$(workdir)
PID=""
D=""
cleanup() {
  if [ -n "${WRITER_PID:-}" ]; then
    touch "${RELEASE:-$W/release-writer}"
    wait "$WRITER_PID" 2>/dev/null || true
  fi
  if [ -n "$PID" ]; then
    kill -CONT "$PID" 2>/dev/null || true
    kill -TERM "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -rf "$W"
  eval "$_stress_state_cleanup"
}
trap cleanup EXIT

# Each control runs the complete witness with its own assertion channel.
# Late apply must report an unproven injection; the unsafe-outcome control
# must make the unchanged assertions reject its simulated faulty result.
run_controls() {
  local control result output
  for control in late ignored; do
    output="$W/control-$control.out"
    env -u STRESS_WRAP_DEPTH -u STRESS_WRAPPER_PID -u STRESS_RESULT_FD \
      -u STRESS_FAILURE_ARTIFACTS /bin/bash "${BASH_SOURCE[0]}" "--control-$control" \
      >"$output" 2>&1 198<&- 199>&-
    result=$?
    if [ "$control" = late ]; then
      if { [ "$result" -eq 2 ] || [ "$result" -eq 0 ]; } && ! grep -q "FAIL" "$output" && ! grep -q "injection proven" "$output" && grep -q 'unproven.*injection' "$output"; then
        pass "late injection control is unproven, never a pass or a product failure"
      else
        fail "late injection control wanted an unproven B injection without product failure, got $result: $(cat "$output")"
      fi
    elif [ "$result" -eq 1 ] && grep -q 'confirmed stopped' "$output" && grep -q 'FAIL' "$output"; then
      pass "paused unsafe-outcome control fails retained assertions after a proven injection"
    else
      fail "paused unsafe-outcome control did not reject the fault (exit $result): $(cat "$output")"
    fi
  done
}

export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
DA="$W/a/Desktop"; mkdir -p "$DA"
for i in $(seq 1 8); do : > "$DA/widget_$(letters "$i").txt"; done
BLOCKER_CONTENT="I-AM-A-FILE-NOT-A-DIRECTORY-$$"
printf '%s' "$BLOCKER_CONTENT" > "$DA/Documents"
BEFORE_A=$(find "$DA" -type f | wc -l | tr -d ' ')

CODE_A=0
OUT_A=$("$SWEEP" apply "$DA" --yes 2>&1) || CODE_A=$?

assert_eq 1 "$([ "$CODE_A" != "0" ] && echo 1 || echo 0)" "[A] apply refused rather than crashing or misbehaving (exit $CODE_A)"
assert_eq "$BLOCKER_CONTENT" "$(cat "$DA/Documents" 2>/dev/null)" "[A] the blocking file's content is untouched. sweep did not write into it or replace it"
assert_eq "$BEFORE_A" "$(find "$DA" -type f | wc -l | tr -d ' ')" "[A] every source file is still present. Nothing was lost trying to reach a broken destination"
if echo "$OUT_A" | grep -qi "sweep:"; then
  pass "[A] the refusal was printed, not swallowed"
else
  fail "[A] apply exited $CODE_A but printed nothing identifiable: $OUT_A"
fi

# --- Variant B: mid-run, a real first group buys the window ---------------

build_two_groups() {  # $1 = Desktop dir
  mkdir -p "$1"
  python3 - "$1" <<'PY'
import sys, os
d = sys.argv[1]
def letters(i, width=5):
    s = ""
    for _ in range(width):
        s = chr(97 + i % 26) + s
        i //= 26
    return s
# Group 1: a camera burst, large, processed first by plan order. This is
# the group that buys real wall-clock time before group 2 is even reached.
for i in range(450):
    open(os.path.join(d, f"IMG_{i:04d}.jpg"), "w").close()
# Group 2: a small Documents group whose destination is blocked while apply
# is confirmed stopped. Text files belong to the Documents type family.
for i in range(8):
    open(os.path.join(d, f"widget_{letters(i)}.txt"), "w").close()
PY
}

INJECTED=0
for trial in 1 2 3 4; do
  DB="$W/b-$trial/Desktop"; build_two_groups "$DB"
  D="$DB"
  export ETUDE_STATE_DIR="$W/state-$trial"
  mkdir -p "$ETUDE_STATE_DIR"
  TARGET=$("$SWEEP" "$DB" --json 2>/dev/null | python3 -c 'import json,sys; g=next(g for g in json.load(sys.stdin)["details"]["groups"] if g["name"]=="Documents"); print(g["members"][-1])')
  DEST="$DB/Documents/$(basename "$TARGET")"
  BEFORE_B=$(find "$DB" -type f | wc -l | tr -d ' ')
  "$SWEEP" apply "$DB" --yes >"$W/b.out" 2>"$W/b.err" &
  PID=$!
  if [ "$CONTROL" = --control-late ]; then wait "$PID" 2>/dev/null || true; fi
  if ! WITNESS=$("$DUMP" --watch-apply "$ETUDE_STATE_DIR" "$D" "$TARGET" "$PID" 2>"$W/watch.err"); then
    kill -CONT "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
    PID=""
    [ "$CONTROL" != --control-late ] || break
    continue
  fi
  read -r JOURNAL_MOVED JOURNAL_TOTAL PENDING_INDEX <<< "$WITNESS"
  STOPPED=$(ps -o state= -p "$PID" 2>/dev/null)
  if [[ "$STOPPED" != *T* ]] || [ ! -f "$TARGET" ] || [ -e "$DEST" ] || [ -L "$DEST" ]; then
    kill -CONT "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
    PID=""
    continue
  fi
  BLOCKER_B="RACE-BLOCKER-$$"
  if ! (set -C; printf '%s' "$BLOCKER_B" > "$DB/Documents") 2>/dev/null; then
    kill -CONT "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
    PID=""
    continue
  fi
  INJECTED=1
  pass "[B] injection proven: exclusively created blocker while apply was confirmed stopped with pending Documents entry ($JOURNAL_MOVED/$JOURNAL_TOTAL journalled moves)"
  if [ "$CONTROL" = --control-ignored ]; then rm "$DB/Documents"; fi
  kill -CONT "$PID" || { fail "could not resume proven blocker injection"; exit 0; }
  wait "$PID"
  CODE_B=$?
  PID=""
  assert_eq 1 "$([ "$CODE_B" != "0" ] && echo 1 || echo 0)" "[B] apply refused rather than succeeding over the blocked destination (exit $CODE_B)"
  assert_eq "$BLOCKER_B" "$(cat "$DB/Documents" 2>/dev/null)" "[B] the mid-run blocking file's content is untouched"

  MOVED_PHOTOS=$(find "$DB" -mindepth 2 -name 'IMG_*.jpg' 2>/dev/null | wc -l | tr -d ' ')
  echo "    ($MOVED_PHOTOS of the first group's files had already landed before the collision)"
  # +1 for the blocker file itself, which we planted and which is expected
  # to still be there. It is not something the tool lost.
  assert_eq "$((BEFORE_B + 1))" "$(find "$DB" -type f | wc -l | tr -d ' ')" "[B] no file was lost anywhere in the tree. The earlier group's real moves stand, nothing else vanished"
  break
done
if [ "$INJECTED" != 1 ]; then
  unproven "[B] destination injection" "no confirmed stopped apply with pending Documents target across four trials: $(cat "$W/watch.err")"
  exit 0
fi
[ -n "$CONTROL" ] || run_controls
true
