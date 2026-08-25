# Emit a check's outcome as a typed causal-trace event. Sourced, not run.
#
# The point is that a trace costs nothing extra. Every check in this repo
# already ends in a verdict with a reason and prints it; this writes the same
# verdict as a typed event on the way past. Nothing here asks anyone to
# narrate what they did, because a trace that depends on being written by
# hand is one that decays the first time the work gets hurried.
#
# Schema is regne.mod.causal-trace.v1 -- the same hash-chained events.jsonl
# modctl reads -- so these traces need no adapter. Kinds are fixed by that
# schema: observation, decision, intervention, verification, failure,
# boundary. A check produces `verification` when it holds and `failure` when
# it does not. Neither is ever written by the thing being checked: the check
# is a separate process from the code it judges, which is the weak form of
# the independence rule modctl enforces properly.
#
# Off unless ETUDES_TRACE names a run directory, so a developer running a
# check by hand is not silently generating research artifacts.
#
#   source scripts/trace.sh
#   trace_event verification "the site's claims match what runs" '{"ok":11}'

trace_event() {
  [ -n "${ETUDES_TRACE:-}" ] || return 0
  local kind="$1" statement="$2" data="${3:-null}"
  python3 - "$ETUDES_TRACE" "$kind" "$statement" "$data" <<'PY'
import hashlib, json, os, sys
from datetime import datetime, timezone
from pathlib import Path

run, kind, statement, raw = Path(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4]
KINDS = {"observation", "decision", "intervention", "verification", "failure", "boundary"}
if kind not in KINDS:
    sys.exit(f"trace: unknown kind {kind}")
run.mkdir(parents=True, exist_ok=True)

def digest(obj):
    # Byte-for-byte modctl's _canonical: sorted keys, compact separators,
    # ensure_ascii=False. Anything else produces a different hash and its
    # reader rejects the trace at sequence 0 -- which is what happened when
    # this used json.dumps defaults, and is the correct outcome. Compatibility
    # asserted in prose is not compatibility; the other implementation's
    # reader is the only judge that counts.
    body = {k: v for k, v in obj.items() if k != "event_sha256"}
    canonical = json.dumps(body, sort_keys=True, separators=(",", ":"),
                           ensure_ascii=False).encode("utf-8")
    return hashlib.sha256(canonical).hexdigest()

log = run / "events.jsonl"
rows = [json.loads(l) for l in log.read_text().splitlines() if l.strip()] if log.exists() else []
# A closed trace stays closed; appending after a terminal boundary would let
# a later run rewrite a finished verdict.
if rows and rows[-1]["kind"] == "boundary" and rows[-1]["payload"].get("terminal"):
    sys.exit("trace: this run is closed")

payload = {"statement": statement}
try:
    data = json.loads(raw)
except ValueError:
    data = None
if isinstance(data, dict):
    payload["data"] = data
# Who produced this, asked of the environment rather than claimed in prose.
payload["source"] = {
    "tool": os.environ.get("ETUDES_TRACE_TOOL", "etudes-check"),
    "agent": os.environ.get("ETUDES_TRACE_AGENT", "unattributed"),
    "ci": bool(os.environ.get("CI")),
}
event = {
    "seq": len(rows),
    "utc": datetime.now(timezone.utc).isoformat(),
    "kind": kind,
    "previous_sha256": rows[-1]["event_sha256"] if rows else None,
    "payload": payload,
}
event["event_sha256"] = digest(event)
with log.open("a", encoding="utf-8") as fh:
    fh.write(json.dumps(event, sort_keys=True) + "\n")
PY
}
