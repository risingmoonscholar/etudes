#!/usr/bin/env bash
# The steps `stash status` prints work when pasted, whatever the folder is named.
# Run from a clear folder, status points at the stash elsewhere and prints how to
# see it and how to restore it. With two stashes of one folder, pop refuses and
# prints how to restore the newest. Each step is run exactly as printed.
#
# The folder names hold a space and a single quote, or an escape character. No
# raw escape byte may reach the terminal, and --json carries the real path.
# The two names each get their own records, so status never names the other.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir) || exit 1; trap 'rm -rf "${W:?}"' EXIT
W=$(cd "${W:?}" && pwd -P) && [ -n "$W" ] || exit 1
require python3 "these checks read JSON with Python" || exit 0
export HOME="$W/home"; mkdir -p "$HOME" "$W/clear"
ESC=$'\033'

fresh_state() {  # a stash of another case must not be the one status names
  export ETUDE_STATE_DIR="$W/$1-state"; mkdir -p "$ETUDE_STATE_DIR"
}
files() {  # files DIR NAME...: one small file per name
  local dir="$1" name; shift
  mkdir -p "$dir"
  for name in "$@"; do printf '%s\n' "$name" >"$dir/$name.txt"; done
}
# held DIR: how many visible items are in DIR (a holding folder is hidden)
held() { find "$1" -mindepth 1 -maxdepth 1 ! -name '.*' | wc -l | tr -d ' '; }
# step LABEL TEXT PREFIX: the printed step after PREFIX on its own line
step() {
  local line
  line=$(printf '%s\n' "$2" | sed -n "s/^ *$3//p" | head -1)
  if [ -n "$line" ]; then pass "$1: step printed"; else fail "$1: no '$3' step in: $2"; fi
  STEP=$line
}
# run_step LABEL: run STEP as printed, from the clear folder
run_step() {
  local out code=0
  [ -n "$STEP" ] || { fail "$1: no step to run"; return; }
  out=$(cd "$W/clear" && stash() { "$STASH" "$@"; } && eval "$STEP" 2>&1 </dev/null) || code=$?
  if [ "$code" = 0 ]; then pass "$1: step runs as printed"; else fail "$1: '$STEP' exit $code: $out"; fi
  no_escape "$1: step output" "$out"
}
no_escape() {
  case "$2" in *"$ESC"*) fail "$1: a raw escape byte reached the terminal" ;; *) pass "$1: no raw escape" ;; esac
}

for case in quote escape; do
  if [ "$case" = quote ]; then name="my desk's"; else name="bad${ESC}[31mname"; fi
  dir="$W/$case/$name"

  # From a clear folder: status names the stash elsewhere.
  fresh_state "$case"
  files "$dir" a b
  "$STASH" "$dir" >/dev/null 2>&1 </dev/null
  assert_eq "0" "$(held "$dir")" "$case: folder is clear after stash"
  OUT=$(cd "$W/clear" && "$STASH" status 2>&1 </dev/null)
  assert_no_panic "$case: status elsewhere" "$OUT"
  no_escape "$case: status elsewhere" "$OUT"
  JSON=$(cd "$W/clear" && "$STASH" status --json 2>/dev/null </dev/null)
  GOT=$(printf '%s' "$JSON" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["details"]["elsewhere"])')
  assert_eq "$dir" "$GOT" "$case: --json carries the real path"
  step "$case: see" "$OUT" "See it with: "
  run_step "$case: see"
  assert_eq "0" "$(held "$dir")" "$case: seeing restores nothing"
  step "$case: restore elsewhere" "$OUT" "Restore it with: "
  run_step "$case: restore elsewhere"
  assert_eq "2" "$(held "$dir")" "$case: restore elsewhere brings both back"

  # Two stashes of one folder: pop refuses and says how to restore the newest.
  "$STASH" "$dir" >/dev/null 2>&1 </dev/null
  files "$dir" c
  "$STASH" "$dir" >/dev/null 2>&1 </dev/null
  OUT=$("$STASH" pop "$dir" 2>&1 </dev/null)
  assert_no_panic "$case: ambiguous pop" "$OUT"
  no_escape "$case: ambiguous pop" "$OUT"
  assert_eq "0" "$(held "$dir")" "$case: ambiguous pop restores nothing"
  step "$case: newest" "$OUT" "Or restore the newest: "
  run_step "$case: newest"
  assert_eq "1" "$(held "$dir")" "$case: the newest stash came back"
  [ -f "$dir/c.txt" ] && pass "$case: the newest is the one restored" || fail "$case: c.txt not restored"
done
