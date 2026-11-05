#!/usr/bin/env bash
# Cross-tool: pack makes an archive for another recipient from a mixed tree,
# unpack extracts it into a fresh folder, and the result is judged against the
# source: ordinary bytes equal, the package tree complete, no sensitive name
# and no Finder litter anywhere in the extracted tree, the archive listing or
# either tool's output. sweep then plans against the extracted folder and
# stash reports on it; both read only, and the folder is unchanged afterwards.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
export ETUDE_STATE_DIR="$W/state"; mkdir -p "$ETUDE_STATE_DIR"
export COPYFILE_DISABLE=1

SOURCE="$W/source"; ARCHIVE="$W/share.zip"; OUT="$W/received"
mkdir -p "$SOURCE/notes" "$SOURCE/Example.app/Contents/MacOS" "$SOURCE/Example.app/Contents/Resources"
printf 'ordinary report\n' > "$SOURCE/report.txt"
dd if=/dev/urandom of="$SOURCE/notes/photo.bin" bs=65536 count=3 status=none
printf 'nested note\n' > "$SOURCE/notes/inner.md"
printf 'plist\n' > "$SOURCE/Example.app/Contents/Info.plist"
dd if=/dev/urandom of="$SOURCE/Example.app/Contents/MacOS/Example" bs=4096 count=2 status=none
printf 'strings\n' > "$SOURCE/Example.app/Contents/Resources/en.lproj"
SENSITIVE=("passport_photo_page.png" "credentials.csv" "bank_statement_march.pdf")
for name in "${SENSITIVE[@]}"; do printf 'synthetic private text\n' > "$SOURCE/$name"; done
printf 'synthetic private text\n' > "$SOURCE/notes/SSN_card_scan.jpg"
printf 'litter\n' > "$SOURCE/.DS_Store"
printf 'litter\n' > "$SOURCE/notes/.DS_Store"
ORDINARY=(report.txt notes/photo.bin notes/inner.md Example.app/Contents/Info.plist Example.app/Contents/MacOS/Example Example.app/Contents/Resources/en.lproj)

snapshot_tree "$SOURCE" "$W/source-before.json"
"$PACK" "$SOURCE" --for other --into "$ARCHIVE" --json >"$W/pack.out" 2>"$W/pack.err" </dev/null
assert_eq 0 "$?" "pack --for other published an archive"
snapshot_tree "$SOURCE" "$W/source-after.json"
assert_snapshot_eq "$W/source-before.json" "$W/source-after.json" "pack left the source unchanged"
excluded=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["details"]["excluded"])' "$W/pack.out" 2>/dev/null)
assert_eq 6 "$excluded" "pack counted four sensitive names and two litter files as exclusions"

"$UNPACK" "$ARCHIVE" --into "$OUT" --json >"$W/unpack.out" 2>"$W/unpack.err" </dev/null
assert_eq 0 "$?" "unpack extracted the archive into a fresh folder"
[ -d "$OUT" ] && pass "extracted folder exists" || { fail "extracted folder is missing"; exit 0; }

leaked=0
for name in "${SENSITIVE[@]}" SSN_card_scan.jpg; do
  [ -n "$(find "$OUT" -name "$name" -print -quit)" ] && leaked=1
  grep -qF -- "$name" "$W/pack.out" "$W/pack.err" "$W/unpack.out" "$W/unpack.err" && leaked=1
  /usr/bin/tar -tf "$ARCHIVE" 2>/dev/null | grep -qF -- "$name" && leaked=1
done
[ "$leaked" -eq 0 ] && pass "no sensitive name in the extracted tree, the archive listing or either tool's output" \
  || fail "a sensitive name reached the recipient or the output"
[ -z "$(find "$OUT" -name .DS_Store -print -quit)" ] && pass "no Finder litter was shipped" || fail ".DS_Store reached the recipient"

for rel in "${ORDINARY[@]}"; do
  if cmp -s "$SOURCE/$rel" "$OUT/$rel"; then pass "bytes equal: $rel"; else fail "bytes differ or missing: $rel"; fi
done
src_pkg=$(cd "$SOURCE/Example.app" && find . | LC_ALL=C sort)
out_pkg=$(cd "$OUT/Example.app" 2>/dev/null && find . | LC_ALL=C sort)
assert_eq "$src_pkg" "$out_pkg" "package tree arrived complete"
assert_eq "${#ORDINARY[@]}" "$(find "$OUT" -type f | wc -l | tr -d ' ')" "the recipient has exactly the ordinary files"

# Other tools read the received folder without moving anything.
snapshot_tree "$OUT" "$W/received-before.json"
"$SWEEP" "$OUT" --json >"$W/sweep.out" 2>"$W/sweep.err" </dev/null; sweep_status=$?
case "$sweep_status" in 0|1) pass "sweep planned against the received folder (exit $sweep_status)";; *) fail "sweep exited $sweep_status on the received folder";; esac
"$STASH" status "$OUT" --json >"$W/stash.out" 2>"$W/stash.err" </dev/null; stash_status=$?
case "$stash_status" in 0|1) pass "stash status read the received folder (exit $stash_status)";; *) fail "stash status exited $stash_status on the received folder";; esac
snapshot_tree "$OUT" "$W/received-after.json"
assert_snapshot_eq "$W/received-before.json" "$W/received-after.json" "sweep's plan and stash's status moved nothing"
