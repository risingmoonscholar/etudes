#!/usr/bin/env bash
# The debris every real macOS folder accumulates: .DS_Store, AppleDouble
# resource-fork shadows (._name), .localized, and the custom-folder-icon
# file whose name is literally "Icon" followed by a raw carriage return
# (no dot prefix. Finder hides it via an attribute flag, not by naming
# convention). None of this is the user's data; none of it should be
# grouped, moved as if meaningful, or allowed to corrupt output formatting.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null

json_get() {
  python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(eval(sys.argv[2]))' "$1" "$2"
}

if ! require python3 "writes the literal Icon<CR> filename unambiguously"; then
  exit 0
fi

W=$(workdir); trap 'rm -rf "$W"' EXIT
export ETUDE_STATE_DIR="$W/state"
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets;print(secrets.token_hex(32))')"
D="$W/Desktop"
mkdir -p "$D"

: > "$D/.DS_Store"
: > "$D/._SomeFile.pdf"
: > "$D/.localized"
python3 -c "
import os
open(os.path.join('$D', 'Icon\r'), 'wb').write(b'ICONBYTES')
"
for i in 0 1 2 3 4; do : > "$D/deck_notes_$i.pdf"; done

# --- Apple's own dotfiles are ignored, not grouped, not counted as scanned ---
json=$("$SWEEP" "$D" --json 2>&1)
scanned=$(python3 -c "import json,sys; print(json.load(sys.stdin)['details']['scanned'])" <<<"$json")
hidden=$(python3 -c "import json,sys; print(json.load(sys.stdin)['details']['skipped']['hidden'])" <<<"$json")
assert_eq 6 "$scanned" "scanned excludes .DS_Store, ._SomeFile.pdf and .localized (5 deck files + Icon<CR>)"
assert_eq 3 "$hidden" "all three dotfiles counted as skipped-hidden"

# --- Icon<CR> does not break JSON output ---
# Read as raw stdin bytes, never through a Python string literal. A source
# embedding of "\r" would get double-interpreted by Python's own escape
# handling before json even saw it. That would test this script's
# quoting instead of sweep's escaping.
valid=$(python3 -c "
import json, sys
try:
    json.load(sys.stdin)['details']
    print('ok')
except Exception as e:
    print(f'INVALID: {e}')
" <<<"$json" 2>&1)
assert_eq "ok" "$valid" "--json stays valid JSON with a carriage-return in a filename"

icon_present=$(python3 -c "
import json, sys
d = json.load(sys.stdin)['details']
paths = d['left_alone']['no_clear_group_paths']
print(any('Icon' in p for p in paths))
" <<<"$json")
assert_eq "True" "$icon_present" "Icon<CR> is reported (left alone, no clear group) rather than silently vanishing"

# --- Round-trip Icon<CR> through a real move (stash) and check the exact
#     bytes, not a terminal rendering of them. A bare \r moves the cursor.
#     'ls' output alone would lie about whether the name survived intact.
S="$W/ScreenShareFolder"
mkdir -p "$S"
python3 -c "
import os
open(os.path.join('$S', 'Icon\r'), 'wb').write(b'ICONBYTES')
"
: > "$S/other.txt"

assert_exit 0 "stash moves a folder containing Icon<CR> without erroring" -- "$STASH" "$S"

byte_exact=$(python3 -c "
import os
names = os.listdir('$S')
holding = [n for n in names if n.startswith('.stash-')]
if not holding:
    print('NO-HOLDING-DIR')
else:
    inner = os.listdir(os.path.join('$S', holding[0]))
    print('Icon\r' in inner)
")
assert_eq "True" "$byte_exact" "the stashed name is byte-exact Icon+CR, not truncated or reinterpreted"

assert_exit 0 "stash pop restores it" -- "$STASH" pop "$S"
popped_exact=$(python3 -c "
import os
print('Icon\r' in os.listdir('$S'))
")
assert_eq "True" "$popped_exact" "Icon<CR> round-trips through stash/pop byte-exact"
popped_content=$(python3 -c "
import os
print(open(os.path.join('$S','Icon\r'),'rb').read().decode())
")
assert_eq "ICONBYTES" "$popped_content" "content survived the round trip untouched"

# Extract a synthetic image, then make a Finder alias to it in a separate
# sweep tree. No alias target needs to be opened by sweep to skip the alias.
python3 -c 'import stat,sys,zipfile; info=zipfile.ZipInfo("picture.jpg"); info.create_system=3; info.external_attr=(stat.S_IFREG | 0o600) << 16; z=zipfile.ZipFile(sys.argv[1], "w"); z.writestr(info, b"synthetic image bytes"); z.close()' "$W/images.zip"
assert_exit 0 "unpack publishes the synthetic alias target" -- "$UNPACK" "$W/images.zip" --into "$W/extracted"
A="$W/Aliases"
mkdir -p "$A"
python3 - "$W/extracted/picture.jpg" "$A" <<'PY'
import pathlib, subprocess, sys
target, tree = map(pathlib.Path, sys.argv[1:])
script = '''on run argv
tell application "Finder"
make new alias file at (POSIX file (item 2 of argv) as alias) to (POSIX file (item 1 of argv) as alias) with properties {name:"image-alias.jpg"}
end tell
end run'''
try:
    result = subprocess.run(["osascript", "-e", script, str(target), str(tree)],
                            stdin=subprocess.DEVNULL, capture_output=True, timeout=15)
    if result.returncode:
        raise RuntimeError(result.stderr.decode(errors="replace").strip())
    print("    fixture  Finder created a real alias")
except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
    print(f"    fixture  Finder unavailable ({error}); writing kIsAlias in FinderInfo")
    alias = tree / "image-alias.jpg"
    alias.write_bytes(b"synthetic opaque alias")
    flags = bytearray(32)
    flags[8:10] = (0x8000).to_bytes(2, "big")
    subprocess.run(["xattr", "-wx", "com.apple.FinderInfo", flags.hex(), str(alias)],
                   stdin=subprocess.DEVNULL, check=True)
for i in range(3):
    (tree / f"ordinary-{i}.jpg").write_bytes(b"ordinary fixture")
(tree / "image-link.jpg").symlink_to(target)
PY
snapshot_tree "$A" "$W/aliases-before.json"
snapshot_tree "$W/extracted" "$W/target-before.json"
"$SWEEP" "$A" --json >"$W/aliases.json"
assert_eq 1 "$(json_get "$W/aliases.json" 'd["details"]["skipped"]["aliases_skipped"]')" "Finder alias has its own skip count"
assert_eq 1 "$(json_get "$W/aliases.json" 'd["details"]["skipped"]["symlinks"]')" "symlink count stays separate"
assert_eq 3 "$(json_get "$W/aliases.json" 'd["details"]["scanned"]')" "only ordinary images become candidates"
"$SWEEP" "$A" >"$W/aliases-human.txt"
if grep -q '1 Finder alias skipped by flag' "$W/aliases-human.txt"; then
  pass "human summary discloses the Finder alias count"
else
  fail "human summary omitted the Finder alias count"
fi
assert_exit 0 "apply organises ordinary images beside the alias" -- "$SWEEP" apply "$A" --yes
assert_exit 0 "undo restores those images" -- "$SWEEP" undo "$A"
snapshot_tree "$A" "$W/aliases-after.json"
snapshot_tree "$W/extracted" "$W/target-after.json"
assert_snapshot_eq "$W/aliases-before.json" "$W/aliases-after.json" "alias, symlink and ordinary images survive apply and undo"
assert_snapshot_eq "$W/target-before.json" "$W/target-after.json" "the alias target stays unchanged"
