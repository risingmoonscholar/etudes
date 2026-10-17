#!/usr/bin/env bash
# A tag apply killed part-way is recoverable: status reports it as interrupted
# with true counts, and repeated `sweep undo` puts every file's exact previous
# tag bytes back.
#
# The folder holds 600 pdf files. Some already carry tags (a mix of lists, in a
# fixed order), most carry none. A personal-looking file, a hidden file and a
# symlink sit beside them and must never be tagged. Applying an exported plan
# that adds one new tag to the 600 takes a couple of seconds, long enough to
# stop part-way.
#
# The kill is witnessed, not timed: the dev-only journal watcher stops apply
# after real journal progress, the OS confirms the stop, the journal is reloaded
# while the process cannot move, and only a real partial state is accepted.
# Only then is the process killed. A window that is missed is `unproven`, tried
# again a bounded number of times, and is never a pass and never a failure.
#
# Held while stopped: the files the journal records as done carry their old
# tags plus the new one; the files it does not carry exactly their old tags;
# nothing is renamed, moved or created. After undo, every file's tag bytes are
# exactly what they were.
#
# Two controls show the checks can fail. A tag someone else changes while apply
# is stopped must make undo leave that file and the byte-exact check fail. An
# apply that already finished must be `unproven`, never a witnessed pass.
#
# Tag values are written and read here with Python's plistlib and the C
# library's xattr calls, never with the code under test. Nothing here is real
# data. Every tree is generated and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null

CONTROL="${1:-}"
case "$CONTROL" in ''|--control-late|--control-changed) ;; *) fail "unknown tags control"; exit 0;; esac

if [ "$(uname -s)" != Darwin ]; then
  unproven "tag apply killed part-way" "Finder tags are a macOS feature"
  exit 0
fi
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
  unproven "tag apply killed part-way" "dev-only journal watcher unavailable"
  exit 0
fi

cat > "$W/tagtool.py" <<'PY'
import ctypes, ctypes.util, json, os, plistlib, sys

NAME = b"com.apple.metadata:_kMDItemUserTags"
libc = ctypes.CDLL(ctypes.util.find_library("c"), use_errno=True)
libc.getxattr.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint32, ctypes.c_int]
libc.getxattr.restype = ctypes.c_ssize_t
libc.setxattr.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint32, ctypes.c_int]
libc.setxattr.restype = ctypes.c_int
NOFOLLOW = 1


def get(path):
    size = libc.getxattr(os.fsencode(path), NAME, None, 0, 0, NOFOLLOW)
    if size < 0:
        return None
    buf = ctypes.create_string_buffer(size)
    got = libc.getxattr(os.fsencode(path), NAME, buf, size, 0, NOFOLLOW)
    return buf.raw[:got]


def put(path, tags):
    data = plistlib.dumps(tags, fmt=plistlib.FMT_BINARY)
    if libc.setxattr(os.fsencode(path), NAME, data, len(data), 0, NOFOLLOW) != 0:
        raise OSError(ctypes.get_errno(), "setxattr")


def tag_map(root):
    out = {}
    for name in sorted(os.listdir(root)):
        value = get(os.path.join(root, name))
        out[name] = None if value is None else value.hex()
    return out


if __name__ == "__main__":
    command = sys.argv[1]
    if command == "dump":
        json.dump(tag_map(sys.argv[2]), open(sys.argv[3], "w"), sort_keys=True)
    elif command == "set":
        put(sys.argv[2], sys.argv[3:])
    elif command == "names":
        value = get(sys.argv[2])
        print("none" if value is None else json.dumps(plistlib.loads(value), ensure_ascii=False))
    elif command == "classify":
        # classify ROOT BEFORE: for each file, is it unchanged, old plus Filed, or something else
        before = json.load(open(sys.argv[3]))
        counts = {"same": 0, "added": 0, "other": 0}
        for name, old in before.items():
            now = get(os.path.join(sys.argv[2], name))
            now = None if now is None else now.hex()
            if now == old:
                counts["same"] += 1
                continue
            old_tags = [] if old is None else plistlib.loads(bytes.fromhex(old))
            new_tags = [] if now is None else plistlib.loads(bytes.fromhex(now))
            if name.endswith(".pdf") and new_tags == old_tags + ["Filed"]:
                counts["added"] += 1
            else:
                counts["other"] += 1
        print(counts["same"], counts["added"], counts["other"])
PY

# make_tree DIR: 600 pdfs with a mix of existing tag lists, plus three kinds of
# file that must never be tagged.
make_tree() {
  python3 - "$1" "$W/tagtool.py" <<'PY'
import importlib.util, os, pathlib, sys
spec = importlib.util.spec_from_file_location("tagtool", sys.argv[2])
tagtool = importlib.util.module_from_spec(spec); spec.loader.exec_module(tagtool)
d = pathlib.Path(sys.argv[1])
d.mkdir(parents=True)
lists = [None, ["Old"], None, ["Red\n6", "Old"], None, None, ["Receipts\n4"], ["a", "b", "c"]]
for i in range(600):
    path = d / f"t-{i:04d}.pdf"
    path.write_text(f"synthetic payload {i}\n")
    tags = lists[i % len(lists)]
    if tags:
        tagtool.put(str(path), tags)
(d / "tax return 2024.pdf").write_text("synthetic personal payload\n")
(d / ".tfix-hidden.txt").write_text("synthetic hidden payload\n")
os.symlink(d / "t-0001.pdf", d / "tfix-link.txt")
PY
}

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
PROVEN=0
for attempt in 1 2 3 4; do
  D="$W/attempt-$attempt/Desktop"
  export ETUDE_STATE_DIR="$W/state-$attempt"
  mkdir -p "$ETUDE_STATE_DIR"
  make_tree "$D"
  snapshot_tree "$D" "$W/before.json"
  python3 "$W/tagtool.py" dump "$D" "$W/before-tags.json"

  "$SWEEP" "$D" --scheme tags --map pdf=Filed --export-plan "$W/plan-$attempt" --json >"$W/plan.json" 2>"$W/plan.err"
  plan_status=$?
  if [ "$plan_status" -ne 0 ] || [ "$(json_get "$W/plan.json" 'd["details"]["tags"][0]["files"]')" != "$N" ]; then
    fail "tag kill: the export plans $N files (exit $plan_status, $(json_get "$W/plan.json" 'd["details"]["tags"][0]["files"]') files)"
    break
  fi
  DIGEST=$(json_get "$W/plan.json" 'd["details"]["binding"]["plan_digest"]')
  python3 "$W/tagtool.py" dump "$D" "$W/after-plan-tags.json"
  cmp -s "$W/before-tags.json" "$W/after-plan-tags.json" \
    && defer ok "plan: wrote no tag" || defer FAIL "plan: the folder's tags changed before apply"
  VICTIM=$(python3 - "$D" <<'PY'
import os, sys
root = os.path.realpath(sys.argv[1])
names = sorted(n for n in os.listdir(root) if n.endswith(".pdf") and n != "tax return 2024.pdf")
print(os.path.join(root, names[len(names) * 3 // 4]))
PY
)

  "$SWEEP" apply --plan "$W/plan-$attempt" --plan-digest "$DIGEST" --yes --json \
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
      fail "tag kill: the watcher reported success without a confirmed stopped apply"
      kill -CONT "$PID" 2>/dev/null || true
      wait "$PID" 2>/dev/null || true
      PID=""
      break
    fi
    # Counted while the process cannot write anything.
    read -r UNCHANGED TAGGED_ON_DISK OTHER <<< "$(python3 "$W/tagtool.py" classify "$D" "$W/before-tags.json")"
    # Apply writes a tag and then records it. A stop between the two is a real
    # state but not the one the count claims are about; while attempts remain,
    # let this apply finish and look for a stop on a record boundary.
    if [ "$JOURNAL_TOTAL" != "$N" ] || { [ "$TAGGED_ON_DISK" != "$JOURNAL_MOVED" ] && [ "$attempt" -lt 4 ]; }; then
      echo "    (attempt $attempt stopped between a write and its record: $TAGGED_ON_DISK tagged on disk, $JOURNAL_MOVED journalled; trying again)"
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
    fail "tag kill: an unwitnessed apply exited $APPLY_STATUS: $(head -c 300 "$W/apply.err")"
    break
  fi
  echo "    attempt  tag kill: attempt $attempt unproven"
  [ "$CONTROL" != --control-late ] || break
done

if [ "$PROVEN" != 1 ]; then
  [ "$FAILED" -gt 0 ] || unproven "tag apply killed part-way" "no real stopped partial state across at most four attempts"
  exit 0
fi

# --- the apply is stopped, and the stop is witnessed ---------------------
flush_deferred
pass "apply: stopped by the OS with $JOURNAL_MOVED of $JOURNAL_TOTAL tag writes journalled (reloaded while stopped)"
assert_eq "$N" "$JOURNAL_TOTAL" "apply: the journal holds one entry per planned file"
if [ "$TAGGED_ON_DISK" -gt 0 ] && [ "$TAGGED_ON_DISK" -lt "$N" ]; then
  pass "apply: the tree is really part-way ($TAGGED_ON_DISK of $N files tagged)"
else
  fail "apply: the tree is not part-way ($TAGGED_ON_DISK of $N files tagged)"
fi
assert_eq 0 "$OTHER" "stopped: every file holds its old tags or its old tags plus exactly one added tag"
assert_eq "$((N + 3 - TAGGED_ON_DISK))" "$UNCHANGED" "stopped: every file not yet tagged is byte-for-byte unchanged (the three that never get a tag included)"
snapshot_tree "$D" "$W/stopped.json"
assert_snapshot_eq "$W/before.json" "$W/stopped.json" "stopped: no name, file byte or folder changed; nothing moved"
python3 - "$D" "$W/tagtool.py" <<'PY' && pass "stopped: the personal, hidden and linked files carry no tag" || fail "stopped: a personal, hidden or linked file was tagged"
import importlib.util, os, sys
spec = importlib.util.spec_from_file_location("tagtool", sys.argv[2])
tagtool = importlib.util.module_from_spec(spec); spec.loader.exec_module(tagtool)
for name in ("tax return 2024.pdf", ".tfix-hidden.txt", "tfix-link.txt"):
    assert tagtool.get(os.path.join(sys.argv[1], name)) is None, name
PY

if [ "$CONTROL" = --control-changed ]; then
  # Someone else retags a file apply has already tagged.
  CHANGED=$(python3 - "$D" "$W/tagtool.py" "$W/before-tags.json" <<'PY'
import importlib.util, json, os, sys
spec = importlib.util.spec_from_file_location("tagtool", sys.argv[2])
tagtool = importlib.util.module_from_spec(spec); spec.loader.exec_module(tagtool)
before = json.load(open(sys.argv[3]))
for name in sorted(before):
    if not name.endswith(".pdf"):
        continue
    now = tagtool.get(os.path.join(sys.argv[1], name))
    if now is not None and now.hex() != before[name]:
        tagtool.put(os.path.join(sys.argv[1], name), ["Changed by hand"])
        print(name)
        break
PY
)
  [ -n "$CHANGED" ] && pass "changed-tags control: retagged a file while apply was confirmed stopped" \
    || fail "changed-tags control found no tagged file to change"
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
python3 "$W/tagtool.py" dump "$D" "$W/killed-tags.json"
"$SWEEP" status "$D" --json >"$W/status.json" 2>"$W/status.err"; CODE=$?
assert_eq 0 "$CODE" "status: finds the interrupted operation by folder (exit $CODE)"
assert_eq interrupted "$(json_get "$W/status.json" 'd["details"]["state"]')" "status: reports the killed apply as interrupted"
STATUS_MOVED=$(json_get "$W/status.json" 'd["details"]["counts"]["moved"]')
STATUS_PLANNED=$(json_get "$W/status.json" 'd["details"]["counts"]["planned"]')
assert_eq "$N" "$(( ${STATUS_MOVED:-0} + ${STATUS_PLANNED:-0} ))" "status: written and still-planned counts add up to the planned files"
read -r _ TAGGED_NOW _ <<< "$(python3 "$W/tagtool.py" classify "$D" "$W/before-tags.json")"
if [ "$STATUS_MOVED" = "$TAGGED_NOW" ] || [ "$CONTROL" = --control-changed ]; then
  pass "status: counts $STATUS_MOVED written, the number of files actually tagged"
elif [ "${STATUS_MOVED:-x}" = "$((TAGGED_NOW + 1))" ] || [ "${STATUS_MOVED:-x}" = "$((TAGGED_NOW - 1))" ]; then
  unproven "status: written count equals the files actually tagged" \
    "apply died between a write and its record: $TAGGED_NOW on disk, $STATUS_MOVED journalled"
else
  fail "status: counts ${STATUS_MOVED:-nothing} written but $TAGGED_NOW files were actually tagged"
fi
OPERATION=$(json_get "$W/status.json" 'd["details"]["operation_id"]')
"$SWEEP" status --id "$OPERATION" --json >"$W/status-id.json" 2>"$W/status-id.err"; CODE=$?
assert_eq "0 interrupted" "$CODE $(json_get "$W/status-id.json" 'd["details"]["state"]')" "status: by journal id agrees with status by folder"
python3 "$W/tagtool.py" dump "$D" "$W/after-status-tags.json"
cmp -s "$W/killed-tags.json" "$W/after-status-tags.json" \
  && pass "status: changed no tag" || fail "status changed a tag"

# --- undo: repeat until it reports nothing is left to reverse -------------
runs=0; undo_code=0
while [ "$runs" -lt 4 ]; do
  "$SWEEP" undo "$D" >"$W/undo-$runs.out" 2>&1; undo_code=$?
  [ "$undo_code" -eq 0 ] || break
  runs=$((runs + 1))
  read -r _ _ OTHER_NOW <<< "$(python3 "$W/tagtool.py" classify "$D" "$W/before-tags.json")"
  if [ "$CONTROL" != --control-changed ] && [ "$OTHER_NOW" != 0 ]; then
    fail "undo run $runs: a file holds tags that are neither its old ones nor its old ones plus the new tag"; break
  fi
done
if [ "$runs" -ge 1 ] && [ "$undo_code" = 1 ] && [ "$CONTROL" != --control-changed ]; then
  pass "undo: restored files in $runs run(s), then reported nothing left to reverse"
elif [ "$CONTROL" = --control-changed ]; then
  pass "undo: ran $runs time(s) with a retagged file in the tree"
else
  fail "undo: ended with exit $undo_code after $runs restoring run(s): $(tr '\n' ' ' < "$W/undo-0.out" | cut -c1-400)"
fi
python3 "$W/tagtool.py" dump "$D" "$W/after-undo-tags.json"
cmp -s "$W/before-tags.json" "$W/after-undo-tags.json" \
  && pass "undo: restored every file's exact previous tag bytes" \
  || fail "undo: restored every file's exact previous tag bytes: some file's tags differ from before apply"
snapshot_tree "$D" "$W/after-undo.json"
assert_snapshot_eq "$W/before.json" "$W/after-undo.json" "undo: every name and byte is as it was"

if [ "$CONTROL" = --control-changed ]; then
  grep -q 'had its tags changed since apply and was left alone' "$W/undo-0.out" "$W/undo-1.out" 2>/dev/null \
    && pass "changed-tags control: undo named the file whose tags changed" \
    || fail "changed-tags control: undo did not name the changed file"
else
  "$SWEEP" status "$D" --json >"$W/status-after.json" 2>"$W/status-after.err"; CODE=$?
  assert_eq "0 reversed" "$CODE $(json_get "$W/status-after.json" 'd["details"]["state"]')" "status: reports the operation as reversed after undo"
fi

# --- controls -----------------------------------------------------------
if [ -z "$CONTROL" ]; then
  for control in late changed; do
    output="$W/control-$control.out"
    env -u STRESS_WRAP_DEPTH -u STRESS_WRAPPER_PID -u STRESS_RESULT_FD \
      -u STRESS_FAILURE_ARTIFACTS /bin/bash "${BASH_SOURCE[0]}" "--control-$control" \
      >"$output" 2>&1 198<&- 199>&-
    result=$?
    if [ "$control" = late ]; then
      if [ "$result" -eq 2 ] && grep -q 'unproven.*tag apply killed part-way' "$output" && \
          ! grep -q 'apply: stopped by the OS' "$output"; then
        pass "finished-apply control is unproven, never a witnessed pass or product failure"
      else
        fail "finished-apply control wanted unproven exit 2, got $result: $(tail -c 600 "$output")"
      fi
    elif [ "$result" -eq 2 ] && grep -q 'unproven.*tag apply killed part-way' "$output"; then
      unproven "changed-tags control" "no stopped partial apply for the retag"
    elif [ "$result" -eq 1 ] && grep -q 'retagged a file while apply was confirmed stopped' "$output" && \
        grep -q 'FAIL.*undo: restored every file.s exact previous tag bytes' "$output" && \
        grep -q 'ok .*changed-tags control: undo named the file whose tags changed' "$output"; then
      pass "changed-tags control fails the byte-exact check and undo names the file it left"
    else
      fail "changed-tags control did not reject recovery (exit $result): $(tail -c 800 "$output")"
    fi
  done
fi
true
