#!/usr/bin/env bash
# The lines sweep prints under a plan say what each step does. The Apply line
# says it scans again, the in-use line is printed once, the Save line names a
# file in the home folder that is not taken yet and is not in the scanned
# folder, and a plan that went stale prints a step that remakes the same plan,
# including the content flag, the routes it used, and a note about routes that matched no file.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir) || exit 1; trap 'rm -rf "${W:?}"' EXIT
W=$(cd "${W:?}" && pwd -P) && [ -n "$W" ] || exit 1
require python3 "these checks read JSON with Python" || exit 0
export HOME="$W/home"; mkdir -p "$HOME"
OLD=202001010101
IN_USE="sweep cannot tell if a file is still in use by a program. Check the plan before you apply it."

has() { case "$2" in *"$3"*) pass "$1" ;; *) fail "$1: wanted '$3' in: $2" ;; esac; }
hasnt() { case "$2" in *"$3"*) fail "$1: found '$3' in: $2" ;; *) pass "$1" ;; esac; }

D="$W/inbox"
mkdir -p "$D"
for n in a b c; do printf '%s' "$n" > "$D/$n.pdf"; done
for n in q1 q2 q3; do printf '%s' "$n" > "$D/$n.zq"; done
touch -t "$OLD" "$D"/*.pdf "$D"/*.zq

# --- the plain scan: in-use line, then Review, Apply and Save with the right words ---
OUT=$("$SWEEP" "$D" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "a scan exits 0"
assert_eq 1 "$(printf '%s\n' "$OUT" | grep -cxF "$IN_USE")" "the in-use line is printed once"
has "Review line" "$OUT" "Review: sweep review $D"
has "Review says it scans again" "$OUT" "This scans again, shows each group, and moves only the groups you accept."
has "Apply line" "$OUT" "Apply:  sweep apply $D --yes"
has "Apply says it can differ from the plan" "$OUT" "This scans again and moves what it finds then. That can differ from this plan."
has "Save line names the home folder" "$OUT" "Save:   sweep $D --export-plan ~/sweep.plan"
has "Save says it saves outside the folder" "$OUT" "saves that plan to ~/sweep.plan, outside this folder."
IN_USE_LINE=$(printf '%s\n' "$OUT" | grep -nxF "$IN_USE" | cut -d: -f1)
APPLY_LINE=$(printf '%s\n' "$OUT" | grep -n '^Apply:' | cut -d: -f1)
[ -n "$IN_USE_LINE" ] && [ -n "$APPLY_LINE" ] && [ "$IN_USE_LINE" -lt "$APPLY_LINE" ] \
  && pass "the in-use line comes before the steps" || fail "the in-use line is not before the Apply step"
[ -f "$D/a.pdf" ] && [ -f "$D/b.pdf" ] && [ -f "$D/c.pdf" ] && pass "the scan moved nothing" || fail "the scan moved a file"

# --- a name already taken in the home folder is skipped ---
: > "$HOME/sweep.plan"
OUT=$("$SWEEP" "$D" 2>&1 </dev/null)
has "Save skips the taken name" "$OUT" "--export-plan ~/sweep-2.plan"
hasnt "Save does not offer the taken name" "$OUT" "--export-plan ~/sweep.plan"

# --- a scan of the home folder cannot save into it ---
for n in h1 h2 h3; do printf '%s' "$n" > "$HOME/$n.pdf"; done; touch -t "$OLD" "$HOME"/*.pdf
OUT=$("$SWEEP" "$HOME" 2>&1 </dev/null)
SAVE=$(printf '%s\n' "$OUT" | sed -n 's/^Save:   //p')
case "$SAVE" in
  *"~/sweep"*) fail "a scan of the home folder offers a file inside it: $SAVE" ;;
  *"--export-plan "*) pass "a scan of the home folder offers a file outside it" ;;
  *) fail "a scan of the home folder printed no Save line: $OUT" ;;
esac

# --- a plan that went stale prints a step that remakes it ---
rm -f "$HOME/sweep.plan" "$HOME"/h?.pdf
"$SWEEP" "$D" --since 0 --inspect-content --map zq=Papers --export-plan "$W/p.plan" --json >"$W/export.json" 2>"$W/export.err" </dev/null
assert_eq 0 "$?" "the plan is exported"
DIGEST=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["details"]["binding"]["plan_digest"])' "$W/export.json")
printf 'd' > "$D/d.pdf"; touch -t "$OLD" "$D/d.pdf"
"$SWEEP" apply --plan "$W/p.plan" --plan-digest "$DIGEST" --yes >"$W/stale.out" 2>"$W/stale.err" </dev/null
assert_eq 2 "$?" "a plan made before the folder changed is refused"
ERR=$(cat "$W/stale.err")
has "the refusal says nothing was moved" "$ERR" "Nothing was moved."
has "the step asks for a new plan file" "$ERR" "--export-plan NEW_PLAN_FILE"
has "the step names --inspect-content" "$ERR" "--inspect-content"
has "the step keeps the route" "$ERR" "--map zq=Papers"
has "the step says to add routes that matched no file" "$ERR" "any --map routes that matched no file."
[ -f "$D/a.pdf" ] && [ ! -e "$D/Documents" ] && pass "the stale apply moved nothing" || fail "the stale apply moved something"
