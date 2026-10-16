#!/usr/bin/env bash
# etude-core is shared by all three tools. After the change that lets sweep add
# Finder tags in place, stash and unpack must behave as before.
#
# A folder sweep has just tagged is stashed and popped: the files return with
# the tags sweep added, in order, byte for byte. Then sweep undo, on a journal
# written before the stash and pop, finds the files home and restores their
# earlier tags without touching anything else. unpack extracts a synthetic
# archive into the same folder to its exact bytes. Everything is synthetic, in
# a temp dir, and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null

if [ "$(uname -s)" != Darwin ]; then
  unproven "stash and unpack beside tag plans" "Finder tags are a macOS feature"
  exit 0
fi
require python3 "writes and decodes tag values independently of the tool" || exit 0

W=$(workdir); trap 'rm -rf "$W"; eval "$_stress_state_cleanup"' EXIT
export TAGATTR=com.apple.metadata:_kMDItemUserTags

tags_of() {
  python3 - "$1" <<'PY'
import json, os, plistlib, subprocess, sys
shown = subprocess.run(["xattr", "-px", os.environ["TAGATTR"], sys.argv[1]], capture_output=True, text=True)
print("none" if shown.returncode else json.dumps(
    plistlib.loads(bytes.fromhex(shown.stdout.replace("\n", "").replace(" ", ""))), ensure_ascii=False))
PY
}
set_tags() {
  python3 - "$@" <<'PY'
import os, plistlib, subprocess, sys
path, *tags = sys.argv[1:]
subprocess.run(["xattr", "-wx", os.environ["TAGATTR"], plistlib.dumps(tags, fmt=plistlib.FMT_BINARY).hex(), path], check=True)
PY
}
json_get() {
  python3 - "$1" "$2" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
print(eval(sys.argv[2], {"d": d}))
PY
}

D="$W/folder"; mkdir -p "$D"
for n in 1 2 3; do printf 'synthetic report %s\n' "$n" > "$D/r$n.pdf"; done
set_tags "$D/r1.pdf" Old
"$SWEEP" "$D" --scheme tags --map pdf=Filed --export-plan "$W/plan.bin" --json >"$W/plan.json" 2>/dev/null
DIGEST=$(json_get "$W/plan.json" 'd["details"]["binding"]["plan_digest"]')
"$SWEEP" apply --plan "$W/plan.bin" --plan-digest "$DIGEST" --yes >"$W/apply.out" 2>&1; CODE=$?
assert_eq 0 "$CODE" "sweep adds the tag (exit $CODE)"
assert_eq '["Old", "Filed"]' "$(tags_of "$D/r1.pdf")" "r1.pdf has Old then Filed"
assert_eq '["Filed"]' "$(tags_of "$D/r2.pdf")" "r2.pdf has Filed"

CODE=0; OUT=$("$STASH" "$D" 2>&1) || CODE=$?
assert_eq 0 "$CODE" "stash clears the folder sweep tagged (exit $CODE)"
VISIBLE=$(find "$D" -mindepth 1 -maxdepth 1 ! -name '.stash-*' | wc -l | tr -d ' ')
assert_eq 0 "$VISIBLE" "no visible item is left in the folder"
CODE=0; "$STASH" pop "$D" >/dev/null 2>&1 || CODE=$?
assert_eq 0 "$CODE" "pop restores them (exit $CODE)"
assert_eq '["Old", "Filed"]' "$(tags_of "$D/r1.pdf")" "r1.pdf came back with Old then Filed"
assert_eq '["Filed"]' "$(tags_of "$D/r3.pdf")" "r3.pdf came back with Filed"

"$SWEEP" undo "$D" >"$W/undo.out" 2>&1; CODE=$?
assert_eq 0 "$CODE" "sweep undo still finds the tagged files after a stash and pop (exit $CODE)"
assert_eq '["Old"]' "$(tags_of "$D/r1.pdf")" "r1.pdf is back to Old"
assert_eq none "$(tags_of "$D/r2.pdf")" "r2.pdf has no tag again"
[ "$(cat "$D/r2.pdf")" = 'synthetic report 2' ] && pass "and every file's bytes are unchanged" || fail "a file's bytes changed"

# unpack still extracts to the exact bytes.
SRC="$W/source"; mkdir -p "$SRC/sub"
printf 'synthetic archive member\n' > "$SRC/member.txt"
printf 'synthetic nested member\n' > "$SRC/sub/nested.txt"
tar -czf "$W/fixture.tar.gz" -C "$SRC" member.txt sub
CODE=0; "$UNPACK" "$W/fixture.tar.gz" --into "$W/out" >/dev/null 2>&1 || CODE=$?
assert_eq 0 "$CODE" "unpack extracts the synthetic archive (exit $CODE)"
[ "$(cat "$W/out/member.txt" 2>/dev/null)" = 'synthetic archive member' ] \
  && [ "$(cat "$W/out/sub/nested.txt" 2>/dev/null)" = 'synthetic nested member' ] \
  && pass "unpack wrote both members with their exact bytes" \
  || fail "unpack did not write the expected members"
true
