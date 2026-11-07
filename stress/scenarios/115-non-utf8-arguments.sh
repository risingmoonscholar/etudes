#!/usr/bin/env bash
# An argument that is not valid UTF-8 never panics a tool. std::env::args()
# does, so every tool reads the raw arguments in one shared place and refuses
# the bad one: exit 2, reason invalid_arguments, nothing changed, and a message
# that names the argument with the bad bytes shown as '?'. APFS refuses such file
# names, so only the argument itself can carry the bytes. Covered for sweep,
# stash and unpack, as a path and as a flag value, in text and --json. A valid
# non-ASCII argument still works.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null

require python3 "reads --json results and checks raw bytes" || exit 0

W=$(workdir)
trap 'rm -rf "$W"' EXIT
W=$(cd "${W:?}" && pwd -P) && [ -n "$W" ] || exit 1
export HOME="$W/home" ETUDE_STATE_DIR="$W/state"
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets;print(secrets.token_hex(32))')"
mkdir -p "$HOME" "$ETUDE_STATE_DIR" "$W/folder"
printf 'x' >"$W/folder/a.txt"

BAD=$(printf 'bad\377\376name')       # 0xff 0xfe are never valid UTF-8
ESC=$(printf 'esc\033[2J\377')        # a control byte next to an invalid one
n=0
check() {  # check LABEL TOOL ARGS...: refused, panic-free, no raw bad byte
  local label="$1"; shift
  n=$((n+1))
  local out="$W/out-$n.bin" code
  "$@" >"$out" 2>&1; code=$?
  assert_eq 2 "$code" "$label: exit"
  assert_no_panic "$label" "$(cat "$out")"
  if python3 -I - "$out" <<'PY'
import sys
data = open(sys.argv[1], "rb").read()
sys.exit(0 if not any(b in data for b in (b"\xff", b"\xfe", b"\x1b")) else 1)
PY
  then pass "$label: no raw invalid or control byte in the output"
  else fail "$label: a raw invalid or control byte reached the output"; fi
  if grep -q 'argument .* has bytes that are not valid text' "$out"; then
    pass "$label: names the problem"
  else fail "$label: message missing: $(head -c 300 "$out")"; fi
  if grep -q 'Nothing was changed' "$out"; then pass "$label: says nothing changed"
  else fail "$label: did not say nothing changed"; fi
}
check_json() {  # check_json LABEL TOOL ARGS...: same, and the envelope says why
  local label="$1"; shift
  check "$label" "$@"
  local js="$W/json-$n.txt"
  "$@" >"$js" 2>/dev/null
  if python3 -I - "$js" <<'PY'
import json, sys
d = json.load(open(sys.argv[1], "rb"))
ok = d["status"] == "refused" and d["reason"]["code"] == "invalid_arguments" and d["schema_version"] == 4
sys.exit(0 if ok else 1)
PY
  then pass "$label: json status refused, reason invalid_arguments"
  else fail "$label: json envelope wrong: $(head -c 300 "$js")"; fi
}

for tool in sweep stash unpack; do
  case "$tool" in
    sweep)  BINARY="$SWEEP" ;;
    stash)  BINARY="$STASH" ;;
    unpack) BINARY="$UNPACK" ;;
  esac
  check "$tool path"                 "$BINARY" "$BAD"
  check "$tool path after a valid one" "$BINARY" "$W/folder" "$BAD"
  check "$tool path with control byte" "$BINARY" "$ESC"
  check "$tool flag value"           "$BINARY" "$W/folder" --since "$BAD"
  check_json "$tool path --json"        "$BINARY" "$BAD" --json
  check_json "$tool flag value --json"  "$BINARY" "$W/folder" --since "$BAD" --json
done

# Nothing was touched, and a valid non-ASCII argument still works.
assert_eq a.txt "$(ls "$W/folder")" "the folder is as it was"
mkdir -p "$W/café"
printf 'x' >"$W/café/b.txt"
for tool in sweep stash unpack; do
  case "$tool" in
    sweep)  BINARY="$SWEEP" ;;
    stash)  BINARY="$STASH" ;;
    unpack) BINARY="$UNPACK" ;;
  esac
  "$BINARY" "$W/café" >"$W/ok-$tool.txt" 2>&1
  assert_no_panic "$tool valid non-ASCII path" "$(cat "$W/ok-$tool.txt")"
  if grep -q 'not valid text' "$W/ok-$tool.txt"; then
    fail "$tool refused a valid non-ASCII path: $(head -c 300 "$W/ok-$tool.txt")"
  else pass "$tool does not call a valid non-ASCII path invalid"; fi
done
"$SWEEP" "$W/café" --since 0 >"$W/ok.txt" 2>&1; code=$?
if [ "$code" != 2 ]; then pass "a valid non-ASCII path is not refused (exit $code)"
else fail "a valid non-ASCII path was refused: $(head -c 300 "$W/ok.txt")"; fi
