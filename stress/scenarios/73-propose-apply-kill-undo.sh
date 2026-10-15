#!/usr/bin/env bash
# A plan written by `sweep propose` moves only the files it names, and a hard
# kill part-way through applying it is recoverable like any other apply.
#
# The folder holds 600 eligible files, plus three kinds that must never move:
# a personal-looking file, a hidden file and a symlink. A proposal names 400 of
# the eligible files (200 into each of two folders that already exist), skips
# 100 by name and leaves 100 unnamed. Apply of those 400 moves takes about three
# seconds, long enough to stop part-way.
#
# The kill is witnessed, not timed: the dev-only journal watcher stops apply
# after real journal progress, the OS confirms the stop, the journal is read
# again, and only a real partial state is accepted. Only then is the process
# killed. A window that is missed is `unproven`, tried again a bounded number of
# times, and is never a pass and never a failure.
#
# Held through the whole run: skipped files, unnamed files, the personal-looking
# file, the hidden file and the symlink are in place with their bytes. After
# `sweep undo`, every original path and byte is back.
#
# Two controls show the checks can fail. A file removed while apply is stopped
# must fail the recovery check. An apply that already finished must be
# `unproven`, never a witnessed pass.
#
# Nothing here is real data. Every tree is generated and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
# No tool in this scenario may wait on a terminal.
exec </dev/null

CONTROL="${1:-}"
case "$CONTROL" in ''|--control-late|--control-missing) ;; *) fail "unknown propose control"; exit 0;; esac

require python3 "builds fixture names and reads --json results" || exit 0

W=$(workdir)
PID=""
cleanup() {
  if [ -n "$PID" ]; then
    kill -CONT "$PID" 2>/dev/null || true
    kill -KILL "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -rf "$W"
  eval "$_stress_state_cleanup"
}
trap cleanup EXIT
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"

DUMP="$(dirname "$SWEEP")/journal-dump"
if [ ! -x "$DUMP" ]; then
  unproven "propose then apply killed part-way" "dev-only journal watcher unavailable"
  exit 0
fi

# 300 screenshot-named files make one group; 300 others have no group. Both
# are eligible. The three ineligible files and the two existing destination
# folders sit beside them.
make_tree() {
  python3 - "$1" <<'PY'
import os, pathlib, sys
d = pathlib.Path(sys.argv[1])
d.mkdir(parents=True)
for i in range(300):
    (d / f"Screenshot 2026-01-01 at 10.00.00 AM (pfix {i}).png").write_text(f"synthetic screenshot payload {i}\n")
    (d / f"pfix-note-{i}.xyz").write_text(f"synthetic note payload {i}\n")
(d / "tax return 2024.pdf").write_text("synthetic personal payload\n")
(d / ".pfix-hidden.txt").write_text("synthetic hidden payload\n")
os.symlink(d / "pfix-note-1.xyz", d / "pfix-link.txt")
(d / "Reading").mkdir()
(d / "Archive").mkdir()
PY
}

# json_get FILE EXPRESSION: evaluate a Python expression over the parsed
# result `d`. Prints nothing and returns 1 when the file is not one result.
json_get() {
  python3 - "$1" "$2" <<'PY'
import json, sys
try:
    d = json.load(open(sys.argv[1]))
    value = eval(sys.argv[2], {"d": d})
except Exception:
    sys.exit(1)
print("" if value is None else value)
PY
}

# make_proposal PLAN_JSON ROOT PROPOSAL ROLES VICTIM: read the exported item
# list, name items by position, write the proposal, and write the role of
# every top-level path. The victim is a file near the end of the apply order.
make_proposal() {
  python3 - "$@" <<'PY'
import json, os, sys
plan_json, root, proposal_path, roles_path, victim_path = sys.argv[1:6]
d = json.load(open(plan_json))
items = d["details"]["items"]
roles, decisions, order, by_folder = {}, [], [], {}
for index, item in enumerate(items):
    slot = index % 6
    name = item["name"]
    if slot <= 3:
        folder = "Reading" if slot <= 1 else "Archive"
        decisions.append({"item": item["id"], "folder": folder, "mode": "require_existing"})
        roles[name] = "move:" + folder
        if folder not in by_folder:
            order.append(folder)
        by_folder.setdefault(folder, []).append(name)
    elif slot == 4:
        decisions.append({"item": item["id"], "skip": True})
        roles[name] = "skip"
    else:
        roles[name] = "unnamed"
roles["tax return 2024.pdf"] = "personal"
roles[".pfix-hidden.txt"] = "hidden"
roles["pfix-link.txt"] = "symlink"
json.dump({"schema_version": 1, "plan_digest": d["details"]["binding"]["plan_digest"],
           "rest": "leave", "decisions": decisions}, open(proposal_path, "w"))
json.dump(roles, open(roles_path, "w"))
# Apply works through the proposal's folders in order of first appearance.
sequence = [name for folder in order for name in by_folder[folder]]
print(len(items), len(sequence))
open(victim_path, "w").write(os.path.join(os.path.realpath(root), sequence[len(sequence) * 3 // 4]))
PY
}

# check_roles ROOT BEFORE ROLES MODE: compare the folder with the snapshot
# taken before apply. Untouched roles must be in place with their bytes in
# every mode. A moved file is at its original path (mode pre, final) or in
# exactly one of its original path and its destination (mode mid).
# Prints "violations moved_at_destination".
check_roles() {
  python3 - "$@" <<'PY'
import json, os, sys
root, before_path, roles_path, mode = sys.argv[1:5]
before = {e["path"]: e for e in json.load(open(before_path))["entries"]}
roles = json.load(open(roles_path))
root = os.path.realpath(root)

def state(rel):
    path = os.path.join(root, rel)
    try:
        info = os.lstat(path)
    except FileNotFoundError:
        return None
    import hashlib, stat
    if stat.S_ISLNK(info.st_mode):
        return ("symlink", os.readlink(path))
    if stat.S_ISREG(info.st_mode):
        with open(path, "rb") as handle:
            return ("file", hashlib.sha256(handle.read()).hexdigest())
    return ("other", None)

def recorded(rel):
    e = before[rel]
    return (e["kind"], e.get("sha256") or e.get("target"))

violations, at_destination = 0, 0
for name, role in roles.items():
    original = recorded(name)
    here = state(name)
    if role.startswith("move:"):
        there = state(os.path.join(role[5:], name))
        if mode == "mid":
            holders = [s for s in (here, there) if s is not None]
            if len(holders) != 1 or holders[0] != original:
                violations += 1
            elif there is not None:
                at_destination += 1
        else:
            if here != original or there is not None:
                violations += 1
    elif here != original:
        violations += 1
print(violations, at_destination)
PY
}

# flush_deferred: report what was set aside until the stop was witnessed.
DEFERRED=()
defer() { DEFERRED+=("$1"$'\t'"$2"); }
flush_deferred() {
  local row
  for row in "${DEFERRED[@]:-}"; do
    [ -n "$row" ] || continue
    if [ "${row%%$'\t'*}" = ok ]; then pass "${row#*$'\t'}"; else fail "${row#*$'\t'}"; fi
  done
  DEFERRED=()
}

N=600
MOVES=400
SKIPS=100
PROVEN=0
for attempt in 1 2 3 4; do
  D="$W/attempt-$attempt/Desktop"
  export ETUDE_STATE_DIR="$W/state-$attempt"
  mkdir -p "$ETUDE_STATE_DIR"
  make_tree "$D"
  snapshot_tree "$D" "$W/before.json"

  "$SWEEP" "$D" --since 0 --export-plan "$W/plan-$attempt" --json >"$W/plan.json" 2>"$W/plan.err"
  plan_status=$?
  if [ "$plan_status" -ne 0 ] || [ "$(json_get "$W/plan.json" 'len(d["details"]["items"])')" != "$N" ]; then
    fail "propose then kill: the export lists $N eligible items (exit $plan_status, $(json_get "$W/plan.json" 'len(d["details"]["items"])') items)"
    break
  fi
  DIGEST=$(json_get "$W/plan.json" 'd["details"]["binding"]["plan_digest"]')
  make_proposal "$W/plan.json" "$D" "$W/proposal.json" "$W/roles.json" "$W/victim.txt" >"$W/proposal.out"
  VICTIM=$(cat "$W/victim.txt")

  snapshot_tree "$D" "$W/before-propose.json"
  "$SWEEP" propose --plan "$W/plan-$attempt" --plan-digest "$DIGEST" --proposal "$W/proposal.json" \
    --export-plan "$W/out-$attempt" --json >"$W/propose.json" 2>"$W/propose.err"
  propose_status=$?
  snapshot_tree "$D" "$W/after-propose.json"
  OUT_DIGEST=$(json_get "$W/propose.json" 'd["details"]["binding"]["plan_digest"]')
  if [ "$propose_status" -ne 0 ] || [ -z "$OUT_DIGEST" ]; then
    fail "propose then kill: sweep propose accepts the proposal (exit $propose_status: $(head -c 300 "$W/propose.err"))"
    break
  fi
  defer ok "propose: the proposal is accepted and writes a bound plan (exit $propose_status)"
  [ "$(json_get "$W/propose.json" 'str(d["details"]["moves"]) + " " + str(d["details"]["skipped"])')" = "$MOVES $SKIPS" ] \
    && defer ok "propose: $MOVES moves and $SKIPS skips, as written" \
    || defer FAIL "propose: wanted $MOVES moves and $SKIPS skips, got $(json_get "$W/propose.json" 'str(d["details"]["moves"]) + " " + str(d["details"]["skipped"])')"
  cmp -s "$W/before-propose.json" "$W/after-propose.json" \
    && defer ok "propose: moved nothing" \
    || defer FAIL "propose: the folder changed before apply"

  "$SWEEP" apply --plan "$W/out-$attempt" --plan-digest "$OUT_DIGEST" --yes --json \
    >"$W/apply.json" 2>"$W/apply.err" &
  PID=$!
  FINISHED_STATUS=""
  if [ "$CONTROL" = --control-late ]; then
    wait "$PID" 2>/dev/null; FINISHED_STATUS=$?
  fi
  WITNESS=""
  if WITNESS=$("$DUMP" --watch-apply "$ETUDE_STATE_DIR" "$D" "$VICTIM" "$PID" 2>"$W/watch.err"); then
    read -r JOURNAL_MOVED JOURNAL_TOTAL _ <<< "$WITNESS"
    STOPPED=$(ps -o state= -p "$PID" 2>/dev/null)
    if [[ "$STOPPED" != *T* ]]; then
      fail "propose then kill: the watcher reported success without a confirmed stopped apply"
      kill -CONT "$PID" 2>/dev/null || true
      wait "$PID" 2>/dev/null || true
      PID=""
      break
    fi
    # Counted while the process cannot move anything. The moved files are the
    # ones now inside the two destination folders.
    MOVED_ON_DISK=$(find "$D/Reading" "$D/Archive" -type f | wc -l | tr -d ' ')
    # Apply moves a file and then records it. A stop between the two is a real
    # state but not the one the count claims are about; while attempts remain,
    # let this apply finish and look for a stop on a record boundary.
    if [ "$JOURNAL_TOTAL" != "$MOVES" ] || { [ "$MOVED_ON_DISK" != "$JOURNAL_MOVED" ] && [ "$attempt" -lt 4 ]; }; then
      echo "    (attempt $attempt stopped between a move and its record: $MOVED_ON_DISK on disk, $JOURNAL_MOVED journalled; trying again)"
      kill -CONT "$PID" 2>/dev/null || true
      wait "$PID" 2>/dev/null || true
      PID=""
      DEFERRED=()
      continue
    fi
    PROVEN=1
    break
  fi
  cat "$W/watch.err" >&2
  kill -CONT "$PID" 2>/dev/null || true
  wait "$PID" 2>/dev/null; APPLY_STATUS=$?
  [ -z "$FINISHED_STATUS" ] || APPLY_STATUS=$FINISHED_STATUS
  PID=""
  DEFERRED=()
  if [ "$APPLY_STATUS" -ne 0 ]; then
    fail "propose then kill: an unwitnessed apply of the proposal exited $APPLY_STATUS: $(head -c 300 "$W/apply.err")"
    break
  fi
  echo "    attempt  propose then kill: attempt $attempt unproven"
  [ "$CONTROL" != --control-late ] || break
done

if [ "$PROVEN" != 1 ]; then
  [ "$FAILED" -gt 0 ] || unproven "propose then apply killed part-way" "no real stopped partial state across at most four attempts"
  exit 0
fi

# --- the apply is stopped, and the stop is witnessed ---------------------
flush_deferred
pass "apply: stopped by the OS with $JOURNAL_MOVED of $JOURNAL_TOTAL moves journalled (reloaded while stopped)"
assert_eq "$MOVES" "$JOURNAL_TOTAL" "apply: the journal holds exactly the proposal's moves, not the whole folder"
if [ "$MOVED_ON_DISK" -gt 0 ] && [ "$MOVED_ON_DISK" -lt "$MOVES" ]; then
  pass "apply: the tree is really part-way ($MOVED_ON_DISK of $MOVES files moved)"
else
  fail "apply: the tree is not part-way ($MOVED_ON_DISK of $MOVES files moved)"
fi
read -r violations at_destination <<< "$(check_roles "$D" "$W/before.json" "$W/roles.json" mid)"
assert_eq 0 "$violations" "stopped: every file is in exactly one place with its bytes; skipped, unnamed, personal, hidden and linked files are untouched"
assert_eq "$MOVED_ON_DISK" "$at_destination" "stopped: every moved file is in the folder its decision named"
top_files=$(find "$D" -maxdepth 1 \( -type f -o -type l \) | wc -l | tr -d ' ')
assert_eq "$((N - MOVED_ON_DISK + 3))" "$top_files" "stopped: the folder still holds the unmoved files plus the three that never move"

if [ "$CONTROL" = --control-missing ]; then
  # A skipped file: it is named by no move, so only a recovery check that
  # compares every path can notice it is gone.
  victim=$(python3 - "$W/roles.json" "$D" <<'PY_PICK'
import json, os, sys
roles = json.load(open(sys.argv[1]))
print(os.path.join(sys.argv[2], sorted(name for name, role in roles.items() if role == "skip")[0]))
PY_PICK
)
  rm -- "$victim"
  pass "missing-file control: removed a file while apply was confirmed stopped"
fi

if kill -9 "$PID" 2>/dev/null; then
  pass "apply: SIGKILL was delivered to the stopped apply"
else
  fail "apply: could not deliver SIGKILL to the stopped apply"
fi
wait "$PID" 2>/dev/null; KILL_STATUS=$?
PID=""
[ "$KILL_STATUS" -ne 0 ] \
  && pass "apply: the killed apply exited non-zero ($KILL_STATUS)" \
  || fail "apply: the killed apply exited zero"

# --- status reports the interrupted operation ----------------------------
snapshot_tree "$D" "$W/killed.json"
"$SWEEP" status "$D" --json >"$W/status.json" 2>"$W/status.err"; CODE=$?
assert_eq 0 "$CODE" "status: finds the interrupted operation by folder (exit $CODE)"
assert_eq interrupted "$(json_get "$W/status.json" 'd["details"]["state"]')" "status: reports the killed apply as interrupted"
STATUS_MOVED=$(json_get "$W/status.json" 'd["details"]["counts"]["moved"]')
STATUS_PLANNED=$(json_get "$W/status.json" 'd["details"]["counts"]["planned"]')
assert_eq "$MOVES" "$(( ${STATUS_MOVED:-0} + ${STATUS_PLANNED:-0} ))" "status: moved and still-planned counts add up to the proposal's moves"
if [ "$STATUS_MOVED" = "$MOVED_ON_DISK" ]; then
  pass "status: counts $STATUS_MOVED moved, the number of files actually moved"
elif [ "${STATUS_MOVED:-x}" = "$((MOVED_ON_DISK + 1))" ] || [ "${STATUS_MOVED:-x}" = "$((MOVED_ON_DISK - 1))" ]; then
  unproven "status: moved count equals the files actually moved" \
    "apply died between a move and its record: $MOVED_ON_DISK on disk, $STATUS_MOVED journalled"
else
  fail "status: counts ${STATUS_MOVED:-nothing} moved but $MOVED_ON_DISK files were actually moved"
fi
OPERATION=$(json_get "$W/status.json" 'd["details"]["operation_id"]')
"$SWEEP" status --id "$OPERATION" --json >"$W/status-id.json" 2>"$W/status-id.err"; CODE=$?
assert_eq "0 interrupted" "$CODE $(json_get "$W/status-id.json" 'd["details"]["state"]')" "status: by journal id agrees with status by folder"
snapshot_tree "$D" "$W/after-status.json"
assert_snapshot_eq "$W/killed.json" "$W/after-status.json" "status: changed no path or byte"
read -r violations _ <<< "$(check_roles "$D" "$W/before.json" "$W/roles.json" mid)"
assert_eq 0 "$violations" "killed: untouched files are still in place and every moved file is in one place"

# --- undo: repeat until it reports nothing is left to reverse -------------
runs=0; undo_code=0
while [ "$runs" -lt 4 ]; do
  "$SWEEP" undo "$D" >"$W/undo-$runs.out" 2>&1; undo_code=$?
  [ "$undo_code" -eq 0 ] || break
  runs=$((runs + 1))
  read -r violations _ <<< "$(check_roles "$D" "$W/before.json" "$W/roles.json" mid)"
  [ "$violations" = 0 ] || { fail "undo run $runs: a file is missing, duplicated or changed, or an untouched file moved"; break; }
done
if [ "$runs" -ge 1 ] && [ "$undo_code" = 1 ]; then
  pass "undo: restored files in $runs run(s), then reported nothing left to reverse"
else
  fail "undo: ended with exit $undo_code after $runs restoring run(s): $(tr '\n' ' ' < "$W/undo-0.out" | cut -c1-400)"
fi
snapshot_tree "$D" "$W/after-undo.json"
assert_snapshot_eq "$W/before.json" "$W/after-undo.json" "undo: restored every original path and byte"
read -r violations _ <<< "$(check_roles "$D" "$W/before.json" "$W/roles.json" final)"
assert_eq 0 "$violations" "undo: every moved file is back at its original path, and the skipped, unnamed, personal, hidden and linked files never moved"

"$SWEEP" status "$D" --json >"$W/status-after.json" 2>"$W/status-after.err"; CODE=$?
assert_eq "0 reversed" "$CODE $(json_get "$W/status-after.json" 'd["details"]["state"]')" "status: reports the operation as reversed after undo"

# --- controls -----------------------------------------------------------
if [ -z "$CONTROL" ]; then
  for control in late missing; do
    output="$W/control-$control.out"
    env -u STRESS_WRAP_DEPTH -u STRESS_WRAPPER_PID -u STRESS_RESULT_FD \
      -u STRESS_FAILURE_ARTIFACTS /bin/bash "${BASH_SOURCE[0]}" "--control-$control" \
      >"$output" 2>&1 198<&- 199>&-
    result=$?
    if [ "$control" = late ]; then
      if [ "$result" -eq 2 ] && grep -q 'unproven.*propose then apply killed part-way' "$output" && \
          ! grep -q 'apply: stopped by the OS' "$output"; then
        pass "finished-apply control is unproven, never a witnessed pass or product failure"
      else
        fail "finished-apply control wanted unproven exit 2, got $result: $(tail -c 600 "$output")"
      fi
    elif [ "$result" -eq 2 ] && grep -q 'unproven.*propose then apply killed part-way' "$output"; then
      unproven "missing-file control" "no stopped partial apply for the deletion"
    elif [ "$result" -eq 1 ] && grep -q 'removed a file while apply was confirmed stopped' "$output" && \
        grep -q 'FAIL.*undo: restored every original path and byte' "$output"; then
      pass "missing-file control fails byte-exact recovery after a deletion while stopped"
    else
      fail "missing-file control did not reject recovery (exit $result): $(tail -c 800 "$output")"
    fi
  done
fi
true
