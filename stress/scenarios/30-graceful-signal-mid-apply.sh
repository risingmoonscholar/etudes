#!/usr/bin/env bash
# Interruption family: SIGINT and SIGTERM mid-apply.
#
# A "graceful" signal should not be worse than kill -9. sweep installs no
# signal handler anywhere in the tree (grep for ctrlc / signal_hook turns up
# nothing), so SIGINT and SIGTERM get the OS default disposition: immediate
# termination, no unwind, no Drop. That means whatever kill -9 can do to an
# in-flight move, Ctrl-C and a plain `kill` can do too. This scenario checks
# that directly, rather than assuming it from reading the source.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
if ! ps -o state= -p $$ >/dev/null 2>&1; then
  unproven "stopped-process witness" "ps is unavailable in this sandbox"
  exit 0
fi
W=$(workdir) || exit 1
trap 'rm -rf "$W"' EXIT

make_tree() {
  local d="$1" n="$2"
  mkdir -p "$d"
  for i in $(seq 1 "$n"); do
    : > "$d/Screenshot 2026-0$((i % 9 + 1))-$(printf %02d $((i % 28 + 1))) at $(printf %02d $((i % 12 + 1))).$(printf %02d $((i % 60))).$(printf %02d $((i % 60))) AM ($i).png"
  done
}

# Same polling-kill approach as the SIGKILL scenario, parameterised on signal.
run_and_signal() {
  local d="$1" target="$2" sig="$3"
  # Reset SIGINT before exec: asynchronous bash jobs otherwise inherit it ignored.
  python3 - "$SWEEP" "$d" <<'PY_EXEC' >"$(dirname "$d")/apply.out" 2>&1 &
import os, signal, sys
signal.signal(signal.SIGINT, signal.SIG_DFL)
os.execv(sys.argv[1], [sys.argv[1], 'apply', sys.argv[2], '--yes'])
PY_EXEC
  local pid=$! moved state
  local deadline=$((SECONDS + 30))
  while kill -0 "$pid" 2>/dev/null; do
    moved=$(find "$d/Screenshots" -type f 2>/dev/null | wc -l | tr -d ' ')
    if [ "$moved" -ge "$target" ]; then
      if stop_confirmed "$pid"; then
        moved=$(find "$d/Screenshots" -type f 2>/dev/null | wc -l | tr -d ' ')
        local remaining; remaining=$(find "$d" -maxdepth 1 -type f | wc -l | tr -d ' ')
        if [ "$moved" -ge "$target" ] && [ "$remaining" -gt 0 ]; then
          # State is frozen and a strict subset has moved before injection.
          kill -"$sig" "$pid" 2>/dev/null || { kill -CONT "$pid"; wait "$pid"; echo missed-window; return; }
          [ "$sig" = KILL ] || kill -CONT "$pid" 2>/dev/null || true
          wait "$pid" 2>/dev/null
          echo signalled
          return
        fi
      fi
      kill -CONT "$pid" 2>/dev/null || true
    fi
    state=$(ps -o state= -p "$pid" 2>/dev/null)
    [[ "$state" != *Z* ]] || break
    if [ "$SECONDS" -ge "$deadline" ]; then
      kill -TERM "$pid" 2>/dev/null || true
      kill -CONT "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null
      echo timed-out
      return
    fi
  done
  wait "$pid" 2>/dev/null
  echo finished-early
}

trial() {
  local sig="$1" target="$2" n="$3"
  local d; d=$(mktemp -d "$W/trial-XXXXXX") || exit 1; d=$d/D
  make_tree "$d" "$n"
  snapshot_tree "$d" "${d%/D}/before.json"
  local before_set; before_set=$(find "$d" -maxdepth 1 -type f -exec basename {} \; | sort)
  local before_n; before_n=$(echo "$before_set" | grep -c .)

  local outcome; outcome=$(run_and_signal "$d" "$target" "$sig")

  "$SWEEP" undo >"${d%/D}/undo.out" 2>&1
  local undo_status=$?
  snapshot_tree "$d" "${d%/D}/after.json"
  local after_set; after_set=$(find "$d" -maxdepth 1 -type f -exec basename {} \; | sort)
  local after_n; after_n=$(find "$d" -type f | wc -l | tr -d ' ')

  if ! cmp -s "${d%/D}/before.json" "${d%/D}/after.json" || [ "$undo_status" -ne 0 ] || [ "$after_set" != "$before_set" ] || [ "$after_n" != "$before_n" ]; then
    echo "FAIL sig=$sig target=$target outcome=$outcome"
    echo "  undo exit=$undo_status (wanted 0); baseline count=$before_n, post-undo total count=$after_n"
    local dupes; dupes=$(comm -12 <(find "$d" -maxdepth 1 -type f -exec basename {} \; | sort) <(find "$d" -mindepth 2 -type f -exec basename {} \; | sort))
    [ -n "$dupes" ] && { echo "  duplicated at both origin and destination, never repaired by undo:"; echo "$dupes" | sed 's/^/    /'; }
    local missing; missing=$(comm -23 <(echo "$before_set") <(echo "$after_set"))
    [ -n "$missing" ] && { echo "  missing from origin after undo:"; echo "$missing" | sed 's/^/    /'; }
    echo "  sweep undo said:"; sed 's/^/    /' "${d%/D}/undo.out"
    rm -f "${d%/D}/undo.out"; rm -rf "$(dirname "$d")"
    return 1
  fi
  rm -f "${d%/D}/undo.out"; rm -rf "$(dirname "$d")"
  echo "TRIAL-OK $outcome"
  return 0
}

run_signal_suite() {
  local sig="$1"; shift
  local total=0 bad=0 hits=0 misses=0 first=""
  for t in "$@"; do
    total=$((total + 1))
    out=$(trial "$sig" "$t" 220); rc=$?
    if [ "$rc" -ne 0 ]; then
      bad=$((bad + 1))
      [ -z "$first" ] && first="$out"
    elif [ "$out" = "TRIAL-OK signalled" ]; then
      hits=$((hits + 1))
    elif [ "$out" = "TRIAL-OK finished-early" ] || [ "$out" = "TRIAL-OK missed-window" ]; then
      misses=$((misses + 1))
    elif [ "$out" = "TRIAL-OK timed-out" ]; then
      bad=$((bad + 1))
      [ -z "$first" ] && first="apply did not reach the signal witness within 10 seconds"
    else
      bad=$((bad + 1))
      [ -z "$first" ] && first="unexpected trial result [$out]"
    fi
  done
  if [ "$bad" -gt 0 ]; then
    fail "SIG$sig mid-apply: $bad/$total trials left the tree wrong after undo. A 'graceful' interrupt (no handler installed) hit the same crash-window defect SIGKILL hits. First reproduction:
$first"
  elif [ "$hits" -eq 0 ]; then
    unproven "SIG$sig mid-apply" "none of the $total named early/middle/late probes reached a live process to signal ($misses missed windows)"
  else
    pass "SIG$sig at $hits/$total named early/middle/late probes ($misses missed windows): undo returned the exact baseline name set"
  fi
}

run_signal_suite INT 1 110 218
run_signal_suite TERM 1 110 218
