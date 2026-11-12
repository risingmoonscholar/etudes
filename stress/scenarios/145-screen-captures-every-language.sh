#!/usr/bin/env bash
# Screen captures are recognised on a Mac in any language, and screen
# recordings get their own group (issues #222 and #223).
#
# One folder holds 3 screenshots with the name a German Mac gives them, 3
# stills with unrelated names that carry the attribute macOS writes on a
# screenshot (com.apple.metadata:kMDItemIsScreenCapture, set here with the
# bytes a real `screencapture -x` wrote on macOS 15.8.2), and 3 English
# screen recordings. sweep plans the 6 stills as Screenshots and the 3
# recordings as Screen Recordings, names the weakest evidence in each group,
# counts one attribute presence read per still and opens no file. Apply moves
# them, the attribute moves with its file, and undo puts every file back.
# A folder of the 3 marked stills alone says "marked by macOS". stash still
# clears and restores a folder byte for byte, and asks for no attribute, and
# unpack, which shares the same libraries, still extracts a synthetic archive
# to its exact bytes. Everything is synthetic, in a temp dir, and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null

if [ "$(uname -s)" != Darwin ]; then
  unproven "screen-capture attribute" "the attribute and xattr(1) are macOS features"
  exit 0
fi
require python3 "these checks read JSON with Python" || exit 0
require xattr "sets the screen-capture attribute independently of the tool" || exit 0

W=$(workdir) || exit 1; trap 'rm -rf "${W:?}"; eval "$_stress_state_cleanup"' EXIT
ATTR=com.apple.metadata:kMDItemIsScreenCapture
# The binary plist `true` that `screencapture -x` wrote on a GitHub macos-15
# runner: https://github.com/risingmoonscholar/etudes/actions/runs/43233341758
MARK=62706c697374303009080000000000000101000000000000000100000000000000000000000000000009
OLD=202001011200

json_get() {
  python3 - "$1" "$2" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
print(eval(sys.argv[2], {"d": d}))
PY
}
GROUP_COUNTS="' '.join(f\"{g['name']}:{g['count']}\" for g in d['details']['groups'])"
# observed FILE CATEGORY: "attempted/observed/failed" from the receipt, or none
observed() {
  json_get "$1" "next(('%d/%d/%d' % (c['attempted'], c['observed'], c['failed']) for c in d['observations']['categories'] if c['category'] == '$2'), 'none')"
}
group_field() {  # group_field FILE GROUP FIELD
  json_get "$1" "next(g['$3'] for g in d['details']['groups'] if g['name'] == '$2')"
}
# The German prefix is "Bildschirmfoto " for a still: the table macOS builds
# names from (screencaptureui.app's Localizable.loctable) writes it before the
# date.
GERMAN=("Bildschirmfoto 2026-01-01 um 09.00.01.png" "Bildschirmfoto 2026-01-02 um 09.00.02.png" \
  "Bildschirmfoto 2026-01-03 um 09.00.03.png")
RECORDINGS=("Screen Recording 2026-01-01 at 9.00.01 AM.mov" "Screen Recording 2026-01-02 at 9.00.02 AM.mov" \
  "Screen Recording 2026-01-03 at 9.00.03 AM.mov")

D="$W/desk"; mkdir -p "$D"
for name in "${GERMAN[@]}" "${RECORDINGS[@]}" notes1.png notes2.png notes3.png; do
  printf 'synthetic %s\n' "$name" > "$D/$name"
done
for n in 1 2 3; do xattr -wx "$ATTR" "$MARK" "$D/notes$n.png"; done
touch -t "$OLD" "$D"/*

# --- the plan -----------------------------------------------------------------
"$SWEEP" "$D" --json >"$W/plan.json" 2>/dev/null; CODE=$?
assert_eq 0 "$CODE" "the plan exits 0"
assert_eq "Screenshots:6 Screen Recordings:3" "$(json_get "$W/plan.json" "$GROUP_COUNTS")" \
  "the 6 stills form Screenshots and the 3 recordings Screen Recordings"
assert_eq "named by macOS as screen captures" "$(group_field "$W/plan.json" Screenshots signal)" \
  "the stills name their weakest evidence: the German names, not the attribute"
assert_eq "still os_name" \
  "$(json_get "$W/plan.json" "next(f\"{g['capture_kind']} {g['capture_basis']}\" for g in d['details']['groups'] if g['name'] == 'Screenshots')")" \
  "the Screenshots group's kind and basis in --json"
assert_eq "named by macOS as screen recordings" "$(group_field "$W/plan.json" "Screen Recordings" signal)" \
  "the recordings are named by macOS"
assert_eq 6/6/0 "$(observed "$W/plan.json" screen_capture_presence)" \
  "one attribute presence read per still, none for a recording, none failed"
assert_eq none "$(observed "$W/plan.json" file_open)" "no file was opened"

OUT=$("$SWEEP" "$D" 2>&1)
case "$OUT" in *"Screen Recordings"*"named by macOS as screen recordings"*) pass "the text plan names the recordings group and its evidence" ;;
  *) fail "the text plan does not show the recordings group: ${OUT%%$'\n'*}" ;; esac

M="$W/marked"; mkdir -p "$M"
for n in 1 2 3; do printf 'synthetic note %s\n' "$n" > "$M/notes$n.png"; xattr -wx "$ATTR" "$MARK" "$M/notes$n.png"; done
touch -t "$OLD" "$M"/*
"$SWEEP" "$M" --json >"$W/marked.json" 2>/dev/null
assert_eq "marked by macOS as screen captures" "$(group_field "$W/marked.json" Screenshots signal)" \
  "3 stills with unrelated names and the attribute are marked by macOS"

# --- apply and undo -----------------------------------------------------------
"$SWEEP" apply "$D" --yes >"$W/apply.out" 2>&1; CODE=$?
assert_eq 0 "$CODE" "apply exits 0"
[ -f "$D/Screenshots/${GERMAN[0]}" ] && [ -f "$D/Screenshots/notes1.png" ] \
  && [ -f "$D/Screen Recordings/${RECORDINGS[0]}" ] \
  && pass "each capture moved into its kind's folder" || fail "a capture is not where its group put it"
[ "$(xattr -px "$ATTR" "$D/Screenshots/notes1.png" 2>/dev/null | tr -d ' \n' | tr 'A-F' 'a-f')" = "$MARK" ] \
  && pass "the attribute moved with its file, unchanged" || fail "a moved still lost its attribute"
"$SWEEP" undo "$D" >"$W/undo.out" 2>&1; CODE=$?
assert_eq 0 "$CODE" "undo exits 0"
[ "$(cat "$D/${RECORDINGS[2]}" 2>/dev/null)" = "synthetic ${RECORDINGS[2]}" ] \
  && [ "$(cat "$D/notes3.png" 2>/dev/null)" = "synthetic notes3.png" ] \
  && [ ! -e "$D/Screenshots" ] && [ ! -e "$D/Screen Recordings" ] \
  && pass "undo put every file back and removed both folders" || fail "undo left the folder changed"

# --- stash shares the scan, and asks for no attribute ------------------------
CODE=0; "$STASH" "$D" --json >"$W/stash.json" 2>/dev/null || CODE=$?
assert_eq 0 "$CODE" "stash runs on the folder (exit $CODE)"
assert_eq none "$(observed "$W/stash.json" screen_capture_presence)" "stash reads no screen-capture attribute"
CODE=0; "$STASH" pop "$D" >/dev/null 2>&1 || CODE=$?
assert_eq 0 "$CODE" "pop restores the folder (exit $CODE)"
[ "$(cat "$D/notes2.png" 2>/dev/null)" = "synthetic notes2.png" ] \
  && pass "the files came back with their bytes" || fail "a file did not come back intact"

# --- unpack shares etude-core and the declaration code ------------------------
SRC="$W/source"; mkdir -p "$SRC"
printf 'synthetic archive member\n' > "$SRC/Bildschirmfoto 2026-01-04 um 09.00.04.png"
tar -czf "$W/fixture.tar.gz" -C "$SRC" .
CODE=0; "$UNPACK" "$W/fixture.tar.gz" --into "$W/out" >/dev/null 2>&1 || CODE=$?
assert_eq 0 "$CODE" "unpack extracts the synthetic archive (exit $CODE)"
[ "$(cat "$W/out/Bildschirmfoto 2026-01-04 um 09.00.04.png" 2>/dev/null)" = 'synthetic archive member' ] \
  && pass "unpack wrote the member with its exact bytes" || fail "unpack did not write the expected member"
true
