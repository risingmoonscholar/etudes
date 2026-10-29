#!/usr/bin/env bash
# A file-size limit makes a real journal append fail after partial progress.
# No next move may start; undo recovers the single unrecorded move and all
# recorded moves. Stash shares the move loop; unpack preserves the history.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null
W=$(workdir)
trap 'rm -rf "$W"; eval "$_stress_state_cleanup"' EXIT
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
export ETUDE_STATE_DIR="$W/state"
mkdir -p "$ETUDE_STATE_DIR" "$W/tree"
python3 -c 'import pathlib,sys; d=pathlib.Path(sys.argv[1]); [(d/f"Screenshot 2026-01-01 at 10.00.00 AM ({i:x}).png").write_text(f"synthetic payload {i}\n") for i in range(40)]' "$W/tree"
snapshot_tree "$W/tree" "$W/before.json"
# Ignore SIGXFSZ so write returns EFBIG and exercises the error path rather
# than killing the process. The base fits; the complete progress stream does not.
(
  trap '' XFSZ
  ulimit -f 64
  "$SWEEP" apply "$W/tree" --since 0 --yes --json >"$W/apply.json" 2>"$W/apply.err"
); code=$?
assert_eq 3 "$code" 'append failure stops apply with incomplete status'
if grep -qi 'file too large' "$W/apply.err"; then
  pass 'append failed with a real file-size write error'
else
  fail "expected file-size write error: $(cat "$W/apply.err")"
fi
"$SWEEP" status "$W/tree" --json >"$W/status.json"; code=$?
assert_eq 0 "$code" 'partial journal remains readable after the failed append'
if python3 -c '
import json,pathlib,sys
s=json.load(open(sys.argv[1]))["details"]
moved=sum(p.is_file() for p in pathlib.Path(sys.argv[2]).glob("Screenshots/*"))
recorded=s["counts"]["moved"]
assert 0 < recorded < 40, (moved,recorded)
assert moved == recorded + 1, (moved,recorded)
assert s["state"] == "interrupted", s
' "$W/status.json" "$W/tree"; then
  pass 'only the failing frame has an unrecorded move; the next file stays home'
else
  fail 'apply moved beyond its durable prefix plus the failing entry'
fi
assert_exit 0 'undo recovers after a failed append' -- "$SWEEP" undo "$W/tree"
snapshot_tree "$W/tree" "$W/recovered.json"
assert_snapshot_eq "$W/before.json" "$W/recovered.json" 'undo restores every original path and byte'
assert_exit 0 'stash uses the shared append loop' -- "$STASH" "$W/tree"
assert_exit 0 'pop restores the shared append loop entries' -- "$STASH" pop "$W/tree"
snapshot_tree "$W/tree" "$W/popped.json"
assert_snapshot_eq "$W/before.json" "$W/popped.json" 'pop restores every original path and byte'
mkdir -p "$W/archive-source" "$W/archive-input"
printf 'synthetic archive payload\n' >"$W/archive-source/payload.txt"
tar -cf "$W/archive-input/example.tar" -C "$W/archive-source" payload.txt
journals=$(shasum "$ETUDE_STATE_DIR"/*.journal)
assert_exit 0 'unpack extracts beside the encrypted history' -- "$UNPACK" "$W/archive-input/example.tar"
assert_eq 'synthetic archive payload' "$(cat "$W/archive-input/example/payload.txt")" 'unpack preserves extracted bytes'
assert_eq "$journals" "$(shasum "$ETUDE_STATE_DIR"/*.journal)" 'unpack preserves journal bytes'
