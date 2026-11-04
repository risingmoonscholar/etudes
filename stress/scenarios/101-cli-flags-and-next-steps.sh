#!/usr/bin/env bash
# A mistyped flag must not move anything, help must answer from any position,
# and a command the tools print for you to run next must do what was shown.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
OLD=202001010101
TOOLS=$(dirname "$SWEEP")

fresh_folder() {  # fresh_folder DIR: three old files at the top, one level down two more
  rm -rf "$1"; mkdir -p "$1/sub"
  printf 'one' > "$1/a.txt"; printf 'two' > "$1/b.txt"; printf 'five' > "$1/e.txt"
  printf 'three' > "$1/sub/c.pdf"; printf 'four' > "$1/sub/d.pdf"
  touch -t "$OLD" "$1/a.txt" "$1/b.txt" "$1/e.txt" "$1/sub/c.pdf" "$1/sub/d.pdf"
}

# --- stash refuses an unknown flag in any position, and moves nothing ---
G="$W/stash me"
for mistake in --dry-run -n --fro --export-pln; do
  fresh_folder "$G"
  assert_exit 2 "stash FOLDER $mistake is refused" -- "$STASH" "$G" "$mistake"
  [ -f "$G/a.txt" ] && pass "stash FOLDER $mistake moved nothing" \
    || fail "stash FOLDER $mistake moved the folder"
done
fresh_folder "$G"
assert_exit 2 "a known flag first does not excuse a later typo" -- "$STASH" --json "$G" --dry-run
[ -f "$G/a.txt" ] && pass "the later typo moved nothing" || fail "the later typo moved the folder"
mkdir -p "$W/second"
assert_exit 2 "stash refuses a second folder" -- "$STASH" "$G" "$W/second"
[ -f "$G/a.txt" ] && pass "the second folder moved nothing" || fail "the second folder moved the first"

# --- help answers from any position, and changes nothing ---
fresh_folder "$G"
for command in "$SWEEP apply --help" "$SWEEP status --help" "$SWEEP review -h" \
               "$STASH pop --help" "$STASH status -h" "$UNPACK --list --help"; do
  label=${command#"$TOOLS"/}
  # shellcheck disable=SC2086 # the binary and its flags are separate words
  OUT=$($command 2>&1); CODE=$?
  assert_eq 0 "$CODE" "$label exits 0"
  case "$OUT" in *USAGE*) pass "$label prints the usage" ;; *) fail "$label printed: ${OUT%%$'\n'*}" ;; esac
done
assert_exit 0 "help after a folder answers" -- "$STASH" "$G" --help
[ -f "$G/a.txt" ] && pass "help after a folder moved nothing" || fail "help after a folder moved it"

# --- the printed Apply command acts on the files the scan showed ---
S="$W/sweep me"
fresh_folder "$S"
LINE=$("$SWEEP" "$S" --depth 2 | sed -n 's/^Apply:  //p')
case "$LINE" in
  *"--depth 2"*) pass "the footer keeps --depth 2" ;;
  *) fail "the footer dropped --depth 2: $LINE" ;;
esac
PATH="$TOOLS:$PATH" bash -c "$LINE" >/dev/null 2>&1
assert_eq 0 "$?" "the printed Apply command runs, on a path with a space"
[ ! -f "$S/a.txt" ] && [ ! -f "$S/sub/c.pdf" ] && pass "it moved the top-level and nested files the scan showed" \
  || fail "it did not move everything the scan showed"
assert_exit 0 "undo restores that apply" -- "$SWEEP" undo "$S"

fresh_folder "$S"
LINE=$("$SWEEP" "$S" --export-plan "$W/plan" | sed -n 's/^Apply this plan: //p')
case "$LINE" in
  *--plan-digest*) pass "after --export-plan the footer names the bound plan" ;;
  *) fail "after --export-plan the footer did not name the plan: $LINE" ;;
esac
PATH="$TOOLS:$PATH" bash -c "$LINE" >/dev/null 2>&1
assert_eq 0 "$?" "the printed plan command runs"
[ ! -f "$S/a.txt" ] && pass "the printed plan command moved the files" || fail "the printed plan command moved nothing"
assert_exit 0 "undo restores the plan apply" -- "$SWEEP" undo "$S"

fresh_folder "$S"
OUT=$("$SWEEP" "$S" --quiet)
case "$OUT" in
  *"$S"*) fail "--quiet printed the path" ;;
  *"sweep apply <path> --yes"*) pass "--quiet keeps a placeholder in the footer" ;;
  *) fail "--quiet footer is missing" ;;
esac

# --- two batches: pop names the choices, and each one works ---
B="$W/batches"
fresh_folder "$B"
"$STASH" "$B" >/dev/null 2>&1
printf 'later' > "$B/later.txt"
"$STASH" "$B" >/dev/null 2>&1
OUT=$("$STASH" pop "$B" 2>&1); CODE=$?
assert_eq 2 "$CODE" "a folder pop with two batches is a refusal"
case "$OUT" in
  *malformed*) fail "the refusal calls a healthy journal malformed" ;;
  *) pass "the refusal does not call the journal malformed" ;;
esac
[ ! -e "$B/a.txt" ] && [ ! -e "$B/later.txt" ] && pass "the refusal restored nothing" \
  || fail "the refusal restored a file"
IDS=$(printf '%s\n' "$OUT" | sed -n 's/^  stash pop --id //p')
assert_eq 2 "$(printf '%s\n' "$IDS" | grep -c .)" "the refusal lists both batch ids"
for id in $IDS; do
  assert_exit 0 "a listed id restores" -- "$STASH" pop --id "$id"
done
[ -f "$B/a.txt" ] && [ -f "$B/later.txt" ] && pass "both batches came back" || fail "a batch did not come back"
