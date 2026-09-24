#!/usr/bin/env bash
# Agent-directed mappings are a distinct contract from project protection:
# a one-run extension route may move ordinary unknown files, but it must never
# outrank a personal-data refusal, accept a partial invalid mapping, or evade
# the journal required for undo.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
MP="$W/Mapped"; mkdir -p "$MP"
for n in a b c; do printf 'model-%s\n' "$n" > "$MP/model_$n.bpy"; done
printf 'tax record\n' > "$MP/tax_return_2025.bpy"
for n in 1 2 3; do printf 'document-%s\n' "$n" > "$MP/doc_$n.pdf"; done
BEFORE="$W/before.json"; snapshot_tree "$MP" "$BEFORE"

CODE=0; MP_OUT=$("$SWEEP" apply "$MP" --map bpy=BlenderBits --yes 2>&1) || CODE=$?
assert_eq 0 "$CODE" "a mapped apply succeeds"
grep -q 'chosen by your agent' <<<"$MP_OUT" \
  && pass "an agent-coined folder name announces itself" \
  || fail "the coined-name disclosure is missing: $MP_OUT"
for n in a b c; do
  [ "$(cat "$MP/BlenderBits/model_$n.bpy" 2>/dev/null)" = "model-$n" ] \
    && pass "mapped model_$n.bpy moved with its original bytes" \
    || fail "mapped model_$n.bpy did not reach BlenderBits exactly"
done
[ -f "$MP/tax_return_2025.bpy" ] && pass "a personal-looking .bpy stayed put; no map outranks a refusal" \
  || fail "an agent mapping moved a personal-looking file"
assert_exit 0 "undo of a mapped apply succeeds" -- "$SWEEP" undo "$MP"
AFTER="$W/after.json"; snapshot_tree "$MP" "$AFTER"
assert_snapshot_eq "$BEFORE" "$AFTER" "undo restored the exact mapping fixture"

# Two valid mappings must both form groups; a duplicate-flag parsing failure
# is not evidence of that contract.
for n in a b c; do printf 'asset-%s\n' "$n" > "$MP/asset_$n.foo"; done
CODE=0; TWO_OUT=$("$SWEEP" "$MP" --map bpy=BlenderBits --map foo=FooBits 2>&1) || CODE=$?
assert_eq 0 "$CODE" "two valid --map flags plan successfully"
grep -q 'BlenderBits' <<<"$TWO_OUT" && grep -q 'FooBits' <<<"$TWO_OUT" \
  && pass "two --map flags form both groups" \
  || fail "repeatable --map is broken: $TWO_OUT"

# One invalid map must refuse before moving anything.
INVALID_BEFORE="$W/invalid-before.json"; snapshot_tree "$MP" "$INVALID_BEFORE"
CODE=0; BAD_OUT=$("$SWEEP" apply "$MP" --map bpy=GoodName --map pdf=Bad --yes 2>&1) || CODE=$?
assert_eq 2 "$CODE" "one invalid map refuses the whole invocation"
grep -q 'pdf already belongs to a built-in group' <<<"$BAD_OUT" \
  && pass "the refusal names the invalid map" \
  || fail "the refusal was not about the bad map: $BAD_OUT"
INVALID_AFTER="$W/invalid-after.json"; snapshot_tree "$MP" "$INVALID_AFTER"
assert_snapshot_eq "$INVALID_BEFORE" "$INVALID_AFTER" "the invalid map changed no path or byte"

CODE=0; "$SWEEP" apply "$MP" --map bpy=GoodName --yes --no-journal >/dev/null 2>&1 || CODE=$?
assert_eq 2 "$CODE" "--map with --no-journal is refused"
