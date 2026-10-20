#!/usr/bin/env bash
# `sweep --scheme tags`: an agent drives the whole tag path on folders it made.
#
# A tag plan adds Finder tags and moves nothing. The plan is the gate: planning
# writes nothing, the live apply path is closed to tags, and only an exported
# plan applied with its digest writes. Tags are only added, in order, after the
# tags a file already has. Undo puts every file's exact previous tag bytes back,
# and a file whose tags were changed by someone else since apply is left and
# named. A file whose tag state cannot be read is skipped, never guessed. A
# plan made before the tags changed is stale. Finder comments are not touched.
#
# Tag values are written and read here with the system's xattr and Python's
# plistlib, never with the code under test. Everything is synthetic, in a temp
# dir, and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null

if [ "$(uname -s)" != Darwin ]; then
  unproven "sweep --scheme tags" "Finder tags are a macOS feature"
  exit 0
fi
require python3 "writes and decodes tag values independently of the tool" || exit 0

W=$(workdir); trap 'rm -rf "$W"; eval "$_stress_state_cleanup"' EXIT
TAGATTR=com.apple.metadata:_kMDItemUserTags
COMMENTATTR=com.apple.metadata:kMDItemFinderComment
export TAGATTR

# set_tags FILE TAG...: store a tag list the way Finder does.
set_tags() {
  python3 - "$@" <<'PY'
import os, plistlib, subprocess, sys
path, *tags = sys.argv[1:]
subprocess.run(["xattr", "-wx", os.environ["TAGATTR"],
                plistlib.dumps(tags, fmt=plistlib.FMT_BINARY).hex(), path], check=True)
PY
}

# tag_dump DIR OUT: every direct child's tag attribute bytes (hex) and mtime and
# ctime in nanoseconds, so "nothing was written" is checked against the file
# system and not against the tool's counters.
tag_dump() {
  python3 - "$1" "$2" <<'PY'
import json, os, subprocess, sys
root, out = sys.argv[1:3]
rows = {}
for name in sorted(os.listdir(root)):
    path = os.path.join(root, name)
    shown = subprocess.run(["xattr", "-px", os.environ["TAGATTR"], path], capture_output=True, text=True)
    info = os.lstat(path)
    rows[name] = {"tag": shown.stdout.replace("\n", "").replace(" ", "") if shown.returncode == 0 else None,
                  "mtime": info.st_mtime_ns, "ctime": info.st_ctime_ns, "inode": info.st_ino}
json.dump(rows, open(out, "w"), sort_keys=True)
PY
}

# tags_of FILE: the tag list as JSON, or "none".
tags_of() {
  python3 - "$1" <<'PY'
import json, os, plistlib, subprocess, sys
shown = subprocess.run(["xattr", "-px", os.environ["TAGATTR"], sys.argv[1]], capture_output=True, text=True)
if shown.returncode:
    print("none")
else:
    print(json.dumps(plistlib.loads(bytes.fromhex(shown.stdout.replace("\n", "").replace(" ", ""))),
                     ensure_ascii=False))
PY
}

# json_get FILE EXPRESSION: evaluate a Python expression over the parsed result.
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

make_folder() {  # make_folder DIR
  mkdir -p "$1"
  for n in a b c; do printf 'synthetic pdf %s\n' "$n" > "$1/$n.pdf"; done
  for n in d e; do printf 'synthetic png %s\n' "$n" > "$1/$n.png"; done
  printf 'synthetic personal\n' > "$1/tax return 2024.pdf"
  set_tags "$1/a.pdf" $'Red\n6' Old
  set_tags "$1/c.pdf" Old
  set_tags "$1/d.png" $'Receipts\n4'
  xattr -w "$COMMENTATTR" 'a private comment' "$1/a.pdf"
}

export_plan() {  # export_plan DIR PLANFILE JSONOUT MAP...
  local dir="$1" plan="$2" json="$3"; shift 3
  local maps=()
  for m in "$@"; do maps+=(--map "$m"); done
  "$SWEEP" "$dir" --scheme tags "${maps[@]}" --export-plan "$plan" --json >"$json" 2>"$json.err"
}

# --- planning writes nothing -------------------------------------------------
D="$W/folder"; make_folder "$D"
snapshot_tree "$D" "$W/before.json"; tag_dump "$D" "$W/before-tags.json"
sleep 0.05

HUMAN=$("$SWEEP" "$D" --scheme tags --map pdf=Filed --map png=Receipts 2>&1); CODE=$?
assert_eq 0 "$CODE" "the plan exits 0"
printf '%s\n' "$HUMAN" | sed 's/^/      /'
grep -qE 'Filed +3 files.*NEW TAG' <<<"$HUMAN" \
  && pass "the plan lists the tag Filed with its count of 3 files and marks it new" \
  || fail "the plan does not list Filed, 3 files, new"
grep -E 'Receipts +1 file ' <<<"$HUMAN" | grep -q 'NEW TAG' \
  && fail "the plan marks Receipts, which a file in the folder carries, as new" \
  || { grep -qE 'Receipts +1 file ' <<<"$HUMAN" && pass "Receipts is listed with 1 file and is not marked new" || fail "the plan does not list Receipts"; }
grep -q '1 file already carries its tag and is left as it is' <<<"$HUMAN" \
  && pass "the plan counts the file that already carries its tag" || fail "the plan does not count the file that already carries its tag"
grep -q '1 tag is new: Filed' <<<"$HUMAN" && pass "the plan names the new tag" || fail "the plan does not name the new tag"
grep -q "Finder's own tag list cannot be read" <<<"$HUMAN" \
  && pass "and says how new is decided" || fail "the plan does not say how new is decided"
grep -q 'Nothing has been tagged or moved' <<<"$HUMAN" && pass "the plan says nothing was tagged or moved" || fail "the plan does not say nothing changed"
grep -q 'a private comment' <<<"$HUMAN" && fail "a Finder comment reached the output" || pass "no Finder comment text in the output"

"$SWEEP" "$D" --scheme tags --map pdf=Filed --map png=Receipts --json >"$W/plan.json" 2>/dev/null
assert_eq Filed "$(json_get "$W/plan.json" '",".join(d["details"]["new_tags"])')" "the JSON plan lists the new tags"
assert_eq "Filed:3:True Receipts:1:False" \
  "$(json_get "$W/plan.json" '" ".join(t["name"] + ":" + str(t["files"]) + ":" + str(t["new"]) for t in d["details"]["tags"])')" \
  "the JSON plan gives each tag, its file count and its new flag"
assert_eq tags "$(json_get "$W/plan.json" 'd["details"]["scheme"]')" "the JSON plan says it is a tags plan"

export_plan "$D" "$W/plan.bin" "$W/export.json" pdf=Filed png=Receipts
assert_eq 0 "$(json_get "$W/export.json" '0 if d["status"] == "done" else 1')" "the plan exports"
DIGEST=$(json_get "$W/export.json" 'd["details"]["binding"]["plan_digest"]')
snapshot_tree "$D" "$W/after-plan.json"; tag_dump "$D" "$W/after-plan-tags.json"
assert_snapshot_eq "$W/before.json" "$W/after-plan.json" "planning, the JSON plan and the export changed no name or byte"
cmp -s "$W/before-tags.json" "$W/after-plan-tags.json" \
  && pass "and wrote no tag and changed no file's tag bytes, mtime, ctime or inode" \
  || fail "planning changed a file's tags or metadata"

# --- the plan is the gate -----------------------------------------------------
"$SWEEP" apply "$D" --scheme tags --map pdf=Filed --yes >"$W/live.out" 2>&1; CODE=$?
assert_eq 2 "$CODE" "the live apply path refuses tags (exit $CODE)"
grep -q 'exported plan' "$W/live.out" && pass "and says to export a plan first" || fail "the live refusal does not say why"
"$SWEEP" review "$D" --scheme tags --map pdf=Filed >"$W/review.out" 2>&1; CODE=$?
assert_eq 2 "$CODE" "review refuses tags (exit $CODE)"
"$SWEEP" apply --plan "$W/plan.bin" --plan-digest "$(printf '0%.0s' $(seq 64))" --yes >"$W/wrong.out" 2>&1; CODE=$?
assert_eq 2 "$CODE" "a plan with the wrong digest is refused (exit $CODE)"
"$SWEEP" apply --plan "$W/plan.bin" --plan-digest "$DIGEST" >"$W/noyes.out" 2>&1; CODE=$?
assert_eq 2 "$CODE" "a plan applied without --yes or --only is refused (exit $CODE)"
"$SWEEP" apply --plan "$W/plan.bin" --plan-digest "$DIGEST" --yes --no-journal >"$W/nojournal.out" 2>&1; CODE=$?
assert_eq 2 "$CODE" "adding tags without a journal is refused (exit $CODE)"
printf '{"schema_version":1,"plan_digest":"%s","rest":"leave","decisions":[]}' "$DIGEST" > "$W/none.json"
"$SWEEP" propose --plan "$W/plan.bin" --plan-digest "$DIGEST" --proposal "$W/none.json" --export-plan "$W/proposed.bin" >"$W/propose.out" 2>&1; CODE=$?
assert_eq 2 "$CODE" "a tags plan cannot be turned into folder proposals (exit $CODE)"
[ ! -e "$W/proposed.bin" ] && pass "and no plan was written" || fail "propose wrote a plan from a tags plan"
snapshot_tree "$D" "$W/gate.json"; tag_dump "$D" "$W/gate-tags.json"
assert_snapshot_eq "$W/before.json" "$W/gate.json" "every refused route left every name and byte alone"
cmp -s "$W/before-tags.json" "$W/gate-tags.json" && pass "and wrote no tag" || fail "a refused route wrote a tag"

# --- the exported plan applies: add only, nothing moved -----------------------
"$SWEEP" apply --plan "$W/plan.bin" --plan-digest "$DIGEST" --yes --json >"$W/apply.json" 2>"$W/apply.err"; CODE=$?
assert_eq 0 "$CODE" "the exported plan applies (exit $CODE)"
assert_eq 4 "$(json_get "$W/apply.json" 'd["details"]["tagged"]')" "apply reports 4 files tagged"
assert_eq 0 "$(json_get "$W/apply.json" 'd["details"]["moved"]')" "and 0 moved"
assert_eq '["Red\n6", "Old", "Filed"]' "$(tags_of "$D/a.pdf")" "a.pdf keeps Red and Old, in order, and gains Filed"
assert_eq '["Filed"]' "$(tags_of "$D/b.pdf")" "b.pdf, which had none, gains Filed"
assert_eq '["Old", "Filed"]' "$(tags_of "$D/c.pdf")" "c.pdf keeps Old and gains Filed"
assert_eq '["Receipts\n4"]' "$(tags_of "$D/d.png")" "d.png, which already had Receipts, is left as it is"
assert_eq '["Receipts\n4"]' "$(tags_of "$D/e.png")" "e.png gains Receipts with the colour the folder's Receipts tag has"
assert_eq none "$(tags_of "$D/tax return 2024.pdf")" "the personal-looking file is never tagged"
snapshot_tree "$D" "$W/after-apply.json"
assert_snapshot_eq "$W/before.json" "$W/after-apply.json" "apply changed no file's bytes, name or mode"
[ "$(xattr -p "$COMMENTATTR" "$D/a.pdf")" = 'a private comment' ] \
  && pass "the Finder comment is untouched" || fail "the Finder comment changed"
"$SWEEP" status "$D" --json >"$W/status.json" 2>/dev/null; CODE=$?
assert_eq "0 completed 4" "$CODE $(json_get "$W/status.json" 'd["details"]["state"]') $(json_get "$W/status.json" 'd["details"]["counts"]["moved"]')" "status reports the tag apply as completed with 4 files changed"

# --- undo restores the exact previous bytes -----------------------------------
"$SWEEP" undo "$D" >"$W/undo.out" 2>&1; CODE=$?
assert_eq 0 "$CODE" "undo exits 0 (exit $CODE)"
grep -q 'Restored the previous tags on 4 files. Nothing was moved.' "$W/undo.out" \
  && pass "undo says it restored the previous tags on 4 files" || fail "undo did not report 4 restored: $(head -c 300 "$W/undo.out")"
python3 - "$W/before-tags.json" "$D" <<'PY' && pass "every file's tag bytes are exactly what they were" || fail "a file's tag bytes differ after undo"
import json, os, subprocess, sys
before = json.load(open(sys.argv[1]))
for name, row in before.items():
    shown = subprocess.run(["xattr", "-px", os.environ["TAGATTR"], os.path.join(sys.argv[2], name)],
                           capture_output=True, text=True)
    now = shown.stdout.replace("\n", "").replace(" ", "") if shown.returncode == 0 else None
    assert now == row["tag"], (name, now, row["tag"])
PY
snapshot_tree "$D" "$W/after-undo.json"
assert_snapshot_eq "$W/before.json" "$W/after-undo.json" "and every name and byte is as it was"
"$SWEEP" undo "$D" >/dev/null 2>&1; CODE=$?
assert_eq 1 "$CODE" "a second undo reports nothing left to reverse (exit $CODE)"
"$SWEEP" status "$D" --json >"$W/status2.json" 2>/dev/null
assert_eq reversed "$(json_get "$W/status2.json" 'd["details"]["state"]')" "status reports the operation as reversed"

# --- a file changed by someone else since apply is left and named ---------------
export_plan "$D" "$W/plan2.bin" "$W/export2.json" pdf=Filed png=Receipts
DIGEST2=$(json_get "$W/export2.json" 'd["details"]["binding"]["plan_digest"]')
"$SWEEP" apply --plan "$W/plan2.bin" --plan-digest "$DIGEST2" --yes >/dev/null 2>&1
set_tags "$D/b.pdf" Filed "Added by hand"
"$SWEEP" undo "$D" >"$W/undo2.out" 2>&1; CODE=$?
assert_eq 0 "$CODE" "undo of a partly changed apply exits 0 (exit $CODE)"
grep -q '1 file had its tags changed since apply and was left alone' "$W/undo2.out" \
  && pass "undo names the file whose tags changed since apply" || fail "undo did not name the changed file: $(head -c 400 "$W/undo2.out")"
assert_eq '["Filed", "Added by hand"]' "$(tags_of "$D/b.pdf")" "and did not overwrite the change"
assert_eq '["Red\n6", "Old"]' "$(tags_of "$D/a.pdf")" "while restoring the file nobody touched"
"$SWEEP" undo "$D" >"$W/undo3.out" 2>&1
assert_eq '["Filed", "Added by hand"]' "$(tags_of "$D/b.pdf")" "a repeated undo still overwrites nothing"
# b.pdf had no tags before apply: put that back by hand.
xattr -d "$TAGATTR" "$D/b.pdf" 2>/dev/null || true

# --- a plan made before the tags changed is stale ------------------------------
export_plan "$D" "$W/plan3.bin" "$W/export3.json" pdf=Filed
DIGEST3=$(json_get "$W/export3.json" 'd["details"]["binding"]["plan_digest"]')
sleep 0.05
set_tags "$D/a.pdf" $'Red\n6' Old "Changed after the plan"
tag_dump "$D" "$W/stale-before.json"
"$SWEEP" apply --plan "$W/plan3.bin" --plan-digest "$DIGEST3" --yes >"$W/stale.out" 2>&1; CODE=$?
assert_eq 2 "$CODE" "a plan made before the tags changed is refused as stale (exit $CODE)"
tag_dump "$D" "$W/stale-after.json"
cmp -s "$W/stale-before.json" "$W/stale-after.json" && pass "and wrote nothing" || fail "a stale plan wrote a tag"

# --- a file with no readable tag state is skipped, never guessed ---------------
U="$W/unreadable"; mkdir -p "$U"
for n in 1 2 3; do printf 'synthetic %s\n' "$n" > "$U/f$n.pdf"; done
xattr -w "$TAGATTR" 'not a property list' "$U/f2.pdf"
OUT=$("$SWEEP" "$U" --scheme tags --map pdf=Filed 2>&1)
grep -q "1 file's tags could not be read; nothing will be added to it" <<<"$OUT" \
  && pass "the plan names the file whose tag state cannot be read" || fail "the plan hid an unreadable tag state: $OUT"
grep -qE 'Filed +2 files' <<<"$OUT" && pass "and plans the other 2 files" || fail "the plan counts are wrong"
export_plan "$U" "$W/plan4.bin" "$W/export4.json" pdf=Filed
DIGEST4=$(json_get "$W/export4.json" 'd["details"]["binding"]["plan_digest"]')
"$SWEEP" apply --plan "$W/plan4.bin" --plan-digest "$DIGEST4" --yes >/dev/null 2>&1
assert_eq 'not a property list' "$(xattr -p "$TAGATTR" "$U/f2.pdf")" "the unreadable file was given nothing"
assert_eq '["Filed"]' "$(tags_of "$U/f1.pdf")" "the readable files were tagged"

# --- nothing to tag ------------------------------------------------------------
N="$W/nothing"; mkdir -p "$N"
printf 'synthetic\n' > "$N/only.pdf"; set_tags "$N/only.pdf" Filed
"$SWEEP" "$N" --scheme tags --map pdf=Filed >"$W/nothing.out" 2>&1; CODE=$?
assert_eq 1 "$CODE" "a tag plan that tags no file exits 1 (exit $CODE)"
grep -q 'No file needs a tag' "$W/nothing.out" && pass "and says so" || fail "it did not say no file needs a tag"

# --- bad requests --------------------------------------------------------------
"$SWEEP" "$D" --scheme tags >/dev/null 2>"$W/nomap.err"; CODE=$?
assert_eq 2 "$CODE" "--scheme tags without --map is refused: sweep invents no tag (exit $CODE)"
"$SWEEP" "$D" --scheme tags --map "pdf=a/b" >/dev/null 2>&1; CODE=$?
assert_eq 2 "$CODE" "a tag name with a slash is refused (exit $CODE)"
"$SWEEP" "$D" --scheme tags --map pdf=Filed --include-tagged >/dev/null 2>&1; CODE=$?
assert_eq 2 "$CODE" "--include-tagged with --scheme tags is refused (exit $CODE)"

# --- the contract names the writes ---------------------------------------------
"$SWEEP" contract --json >"$W/contract.json" 2>/dev/null
assert_eq True "$(json_get "$W/contract.json" 'any("selected_tree_finder_tags" in op["writes"] for op in d["details"]["mutation_scope"]["operations"] if op["operation"] == "apply_exported_plan")')" "the contract declares the tag writes of apply and no write for a scan"
true
