#!/usr/bin/env bash
# The steps `stash status` prints work when pasted, whatever the folder is named.
# Run from a clear folder, status points at the stash elsewhere and prints how to
# see it and how to restore it; when that folder holds two stashes, the step
# restores the newest, since a bare pop of it refuses. With two stashes of one
# folder, pop refuses and prints how to restore the newest. Each step is run
# exactly as printed. A folder whose only stash has no undo record (made with
# --no-journal) is not one pop can restore, so status from elsewhere does not
# offer it, and status in that folder prints no pop step, only where the items
# are and that they move back by hand.
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
has() { case "$2" in *"$3"*) pass "$1" ;; *) fail "$1: wanted '$3' in: $2" ;; esac; }
hasnt() { case "$2" in *"$3"*) fail "$1: found '$3' in: $2" ;; *) pass "$1" ;; esac; }
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
  OUT=$(cd "$W/clear" && "$STASH" status 2>&1 </dev/null)
  assert_no_panic "$case: status elsewhere, two stashes" "$OUT"
  no_escape "$case: status elsewhere, two stashes" "$OUT"
  hasnt "$case: no bare pop for two stashes" "$OUT" "Restore it with:"
  step "$case: see two" "$OUT" "See them with: "
  run_step "$case: see two"
  assert_eq "0" "$(held "$dir")" "$case: seeing two restores nothing"
  step "$case: restore newest elsewhere" "$OUT" "Restore the newest with: "
  run_step "$case: restore newest elsewhere"
  assert_eq "1" "$(held "$dir")" "$case: the newest of two came back from elsewhere"
  [ -f "$dir/c.txt" ] && pass "$case: newest from elsewhere is c" || fail "$case: c.txt not restored from elsewhere"
  "$STASH" "$dir" >/dev/null 2>&1 </dev/null  # c goes back: two stashes again

  OUT=$("$STASH" pop "$dir" 2>&1 </dev/null)
  assert_no_panic "$case: ambiguous pop" "$OUT"
  no_escape "$case: ambiguous pop" "$OUT"
  assert_eq "0" "$(held "$dir")" "$case: ambiguous pop restores nothing"
  step "$case: newest" "$OUT" "Or restore the newest: "
  run_step "$case: newest"
  assert_eq "1" "$(held "$dir")" "$case: the newest stash came back"
  [ -f "$dir/c.txt" ] && pass "$case: the newest is the one restored" || fail "$case: c.txt not restored"
  OUT=$(cd "$W/clear" && "$STASH" status 2>&1 </dev/null)
  step "$case: restore the last" "$OUT" "Restore it with: "
  run_step "$case: restore the last"
  assert_eq "3" "$(held "$dir")" "$case: every file is back"

  # Every record of the folder is now restored. A stash with no undo record
  # is one pop cannot restore, so status from elsewhere must not offer it.
  "$STASH" "$dir" --no-journal >/dev/null 2>&1 </dev/null
  assert_eq "0" "$(held "$dir")" "$case: folder is clear after stash --no-journal"
  OUT=$(cd "$W/clear" && "$STASH" status 2>&1 </dev/null)
  assert_no_panic "$case: status with only an unrecorded stash" "$OUT"
  no_escape "$case: status with only an unrecorded stash" "$OUT"
  hasnt "$case: no step pop cannot follow" "$OUT" "stash pop"

  # From that folder, status says pop cannot restore it and where the items
  # are, as a path that names the real holding folder when pasted.
  real=$(find "$dir" -mindepth 1 -maxdepth 1 -name '.stash-*' -type d)
  OUT=$(cd "$dir" && "$STASH" status 2>&1 </dev/null)
  assert_no_panic "$case: status on an unrecorded stash" "$OUT"
  no_escape "$case: status on an unrecorded stash" "$OUT"
  has "$case: says pop cannot restore it" "$OUT" "so stash pop cannot restore it."
  hasnt "$case: no pop step on an unrecorded stash" "${OUT//so stash pop cannot/}" "stash pop"
  hasnt "$case: no restore step on an unrecorded stash" "$OUT" "Restore with"
  step "$case: items are in" "$OUT" "The items are in: "
  shown_path=$(eval "printf '%s' $STEP")
  assert_eq "$real" "$shown_path" "$case: the printed holding path is the real one"
  has "$case: says to move them back" "$OUT" "Move them back by hand."
  JSON=$(cd "$dir" && "$STASH" status --json 2>/dev/null </dev/null)
  GOT=$(printf '%s' "$JSON" | python3 -I -c 'import json,sys; d=json.load(sys.stdin)["details"]; print(d["holding"]); print(d["undo_record"])')
  assert_eq "$real"$'\n'"False" "$GOT" "$case: --json gives the real holding path and no undo record"
  hasnt "$case: --json has no pop step" "$JSON" "stash pop"
done
