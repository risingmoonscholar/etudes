#!/usr/bin/env bash
# A folder or archive the system will not let a tool read gets an error that names
# the folder and what to do next. It is never reported as clear or as having no work.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir)
trap 'chmod -R u+rwx "$W" 2>/dev/null; rm -rf "$W"' EXIT
if [ "$(id -u)" = 0 ]; then
  unproven "permission-denied messages" "root ignores file modes"
  exit 0
fi

fresh_folder() {  # fresh_folder DIR: three loose files and one subfolder
  chmod -R u+rwx "$1" 2>/dev/null; rm -rf "$1"; mkdir -p "$1/sub"
  printf 'a' > "$1/a.pdf"; printf 'b' > "$1/b.pdf"; printf 'c' > "$1/c.pdf"
}

denied_clearly() {  # denied_clearly LABEL WANT_EXIT -- CMD...: the message names the folder and the next step
  local label="$1" want="$2"; shift 3
  local out code
  out=$("$@" 2>&1 </dev/null); code=$?
  assert_no_panic "$label" "$out"
  assert_eq "$want" "$code" "$label exits $want"
  case "$out" in
    *"cannot read"*"$FOLDER_NAME"*"permission denied"*"ls -ld"*) pass "$label names the folder and the next step" ;;
    *) fail "$label said: ${out%%$'\n'*}" ;;
  esac
  case "$out" in
    *"already clear"*|*"needs organising"*|*"replan"*) fail "$label misreports the cause: ${out%%$'\n'*}" ;;
    *) pass "$label does not misreport the cause" ;;
  esac
}

# --- the folder itself cannot be read ---
D="$W/locked"; FOLDER_NAME=locked
fresh_folder "$D"; chmod 000 "$D"
denied_clearly "sweep scan" 3 -- "$SWEEP" "$D"
denied_clearly "sweep --export-plan" 3 -- "$SWEEP" "$D" --export-plan "$W/plan"
denied_clearly "sweep apply" 3 -- "$SWEEP" apply "$D" --yes
denied_clearly "sweep review" 3 -- "$SWEEP" review "$D"
denied_clearly "stash" 3 -- "$STASH" "$D"
OUT=$("$SWEEP" "$D" --json 2>&1 </dev/null)
assert_no_panic "sweep --json" "$OUT"
OUT=$("$SWEEP" "$D" --json 2>/dev/null </dev/null)
case "$OUT" in
  *'"code":"permission_denied"'*) pass "the JSON reason is permission_denied" ;;
  *) fail "the JSON reason is not permission_denied: ${OUT:0:120}" ;;
esac
[ ! -e "$W/plan" ] && pass "a denied scan writes no plan" || fail "a denied scan wrote a plan"
chmod 755 "$D"
assert_intact "$D" 3 "the denied commands moved nothing"

# --- only a subfolder cannot be read: stash must not call the folder clear ---
S="$W/partly"
fresh_folder "$S"; rm -f "$S"/*.pdf; chmod 000 "$S/sub"
OUT=$("$STASH" "$S" 2>&1 </dev/null); CODE=$?
assert_no_panic "stash with only an unreadable item" "$OUT"
assert_eq 3 "$CODE" "stash exits 3 when the only item cannot be read"
case "$OUT" in
  *"NOT clear"*) pass "stash says the folder is not clear" ;;
  *) fail "stash said: ${OUT%%$'\n'*}" ;;
esac
chmod 755 "$S/sub"
fresh_folder "$S"; chmod 000 "$S/sub"
OUT=$("$STASH" "$S" 2>&1 </dev/null); CODE=$?
assert_no_panic "stash with one unreadable item" "$OUT"
assert_eq 3 "$CODE" "stash exits 3 when one item of several cannot be read"
case "$OUT" in
  *"could not be read"*"NOT clear"*) pass "stash says what stayed and that the folder is not clear" ;;
  *) fail "stash said: $OUT" ;;
esac
chmod 755 "$S/sub"

# --- a status with no history says so ---
OUT=$("$SWEEP" status "$W/partly" 2>&1 </dev/null)
assert_no_panic "sweep status with no history" "$OUT"
case "$OUT" in
  *"on record"*) pass "sweep status with no history says so" ;;
  *) fail "sweep status with no history said: '${OUT%%$'\n'*}'" ;;
esac

# --- unpack: the archive and the destination ---
mkdir -p "$W/z" "$W/readonly"; printf 'h' > "$W/z/a"
(cd "$W/z" && /usr/bin/zip -q ../a.zip a)
cp "$W/a.zip" "$W/locked.zip"; chmod 000 "$W/locked.zip"; chmod 555 "$W/readonly"
for args in "$W/locked.zip" "$W/a.zip --into $W/readonly/out"; do
  # shellcheck disable=SC2086 # the fixture paths hold no spaces
  OUT=$("$UNPACK" $args 2>&1 </dev/null); CODE=$?
  assert_no_panic "unpack when denied" "$OUT"
  assert_eq 3 "$CODE" "unpack exits 3 when denied"
  case "$OUT" in
    *"ls -ld"*) pass "unpack says what to do when denied" ;;
    *) fail "unpack said: ${OUT%%$'\n'*}" ;;
  esac
done

# --- unpack names the archive and destination it was given ---
OUT=$("$UNPACK" "$W/missing-archive.zip" 2>&1 </dev/null)
assert_no_panic "unpack on a missing archive" "$OUT"
case "$OUT" in
  *"no such file"*"missing-archive.zip"*) pass "unpack names a missing archive" ;;
  *) fail "unpack said: ${OUT%%$'\n'*}" ;;
esac
OUT=$("$UNPACK" "$W/locked.zip" 2>&1 </dev/null)
assert_no_panic "unpack on a denied archive" "$OUT"
case "$OUT" in
  *"not a file"*) fail "unpack called an existing archive not a file: ${OUT%%$'\n'*}" ;;
  *) pass "unpack does not call a denied archive not a file" ;;
esac
# The destination's folder is read-only: the reason is the denial, not a crash.
OUT=$("$UNPACK" "$W/a.zip" --into "$W/readonly/out" --json 2>/dev/null </dev/null)
case "$OUT" in
  *'"code":"permission_denied"'*) pass "a read-only destination is permission_denied" ;;
  *) fail "a read-only destination said: ${OUT:0:160}" ;;
esac
OUT=$("$UNPACK" "$W/a.zip" --into "$W/readonly/out" 2>&1 </dev/null)
case "$OUT" in
  *"cannot write to $W/readonly: permission denied. Nothing was extracted."*"--into DIR"*) pass "a read-only destination names the folder and a way out" ;;
  *) fail "a read-only destination said: ${OUT%%$'\n'*}" ;;
esac
chmod 644 "$W/locked.zip"; chmod 755 "$W/readonly"
mkdir -p "$W/taken"
OUT=$("$UNPACK" "$W/a.zip" --into "$W/taken" 2>&1 </dev/null)
assert_no_panic "unpack onto a taken destination" "$OUT"
case "$OUT" in
  *"taken already exists"*) pass "unpack names the destination that exists" ;;
  *) fail "unpack said: ${OUT%%$'\n'*}" ;;
esac
HOSTILE="$W/esc"$'\033'"[31m.zip"
OUT=$("$UNPACK" "$HOSTILE" 2>&1 </dev/null)
assert_no_panic "unpack on a hostile name" "$OUT"
case "$OUT" in
  *$'\033'*) fail "unpack printed an escape code from a filename" ;;
  *) pass "unpack replaces control characters in a name it prints" ;;
esac
