#!/usr/bin/env bash
# Every pack failure says what failed, that no archive was made, and a next step
# that works, in plain words and without a panic. It refuses what it should:
# links out of the folder, credential and system folders, the home folder, a
# destination inside the folder, bad arguments. Help answers from any position.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir)
cleanup() { chmod -R u+rwx "${W:?}" 2>/dev/null; rm -rf "${W:?}"; }
trap cleanup EXIT
W=$(cd "$W" && pwd -P)
export HOME="$W/home" ETUDE_STATE_DIR="$W/state"
mkdir -p "$HOME" "$ETUDE_STATE_DIR"
cd "$W" || exit 0

BANNED='replan|journal seal|malformed|observations|listing failed|extractor|staging|publish|holding locator|holding parent|object identity|member row|dispatched|os error|panicked|[^0-9]1 items|[^0-9]1 files|file\(s\)|item\(s\)|violates|preflight'
clean() {
  assert_no_panic "$1" "$2"
  if printf '%s' "$2" | grep -Eqi "$BANNED"; then fail "$1: internal word in: $2"; else pass "$1: plain words"; fi
}
has() { case "$2" in *"$3"*) pass "$1" ;; *) fail "$1: wanted '$3' in: $2" ;; esac; }
hasnt() { case "$2" in *"$3"*) fail "$1: found '$3' in: $2" ;; *) pass "$1" ;; esac; }
field() { python3 -I -c 'import json,sys; v=json.load(open(sys.argv[2]))
for k in sys.argv[1].split("."): v=v[k]
print(v)' "$1" "$2"; }
# say LABEL EXIT STATUS CODE WORDS -- ARGS...: text, then --json. LAST holds the text.
say() {
  local label="$1" exit="$2" status="$3" code="$4" words="$5"; shift 6
  local out rc
  out=$("$PACK" "$@" 2>&1 </dev/null); rc=$?
  clean "$label" "$out"
  assert_eq "$exit" "$rc" "$label: exit"
  has "$label: words" "$out" "$words"
  "$PACK" "$@" --json >"$W/out.json" 2>"$W/out.err" </dev/null
  assert_eq "$status" "$(field status "$W/out.json")" "$label: status"
  assert_eq "$code" "$(field reason.code "$W/out.json")" "$label: reason"
  clean "$label: json stderr" "$(cat "$W/out.err")"
  LAST="$out"
}
nothing_new() {  # nothing_new LABEL: no archive or unfinished file anywhere here
  local found; found=$(find "$W" -maxdepth 2 \( -name '*.partial' -o -name '*.zip' -o -name '*.tar.gz' \) -print -quit)
  [ -z "$found" ] && pass "$1: no archive and no unfinished file" || fail "$1: left $found"
}

S="$W/source"; mkdir -p "$S"; printf 'bytes\n' > "$S/a.txt"

# --- arguments ----------------------------------------------------------------
say "no folder" 2 refused invalid_arguments "give a folder to pack" -- --for other
say "empty folder name" 2 refused invalid_arguments "the folder name is empty" -- "" --for other
say "no recipient" 2 refused invalid_arguments "say who the archive is for" -- "$S"
say "unknown recipient" 2 refused invalid_arguments 'got "friend"' -- "$S" --for friend
say "empty recipient" 2 refused invalid_arguments "--for needs a value" -- "$S" --for ""
say "empty destination" 2 refused invalid_arguments "--into needs a value" -- "$S" --for other --into ""
say "a flag where a value goes" 2 refused invalid_arguments "--into needs a value" -- "$S" --for other --into --list
say "repeated --for" 2 refused invalid_arguments "--for was given more than once" -- "$S" --for other --for self
say "repeated --into" 2 refused invalid_arguments "--into was given more than once" -- "$S" --for other --into a.zip --into b.zip
say "repeated --format" 2 refused invalid_arguments "--format was given more than once" -- "$S" --for other --format zip --format zip
say "repeated --metadata" 2 refused invalid_arguments "--metadata was given more than once" -- "$S" --for self --metadata keep --metadata drop
say "repeated --max-size" 2 refused invalid_arguments "--max-size was given more than once" -- "$S" --for other --max-size 1G --max-size 2G
say "repeated --list" 2 refused invalid_arguments "--list was given more than once" -- "$S" --for other --list --list
say "a second folder" 2 refused invalid_arguments 'unexpected word "second"' -- "$S" second --for other
say "unknown option" 2 refused unknown_option "unknown option --nope" -- "$S" --for other --nope
say "unsupported format" 2 refused unsupported_format 'not "rar"' -- "$S" --for other --format rar
say "bad metadata" 2 refused invalid_arguments 'got "some"' -- "$S" --for other --metadata some
say "zip with keep" 2 refused invalid_arguments "Use --format tar.gz, or --metadata drop" -- "$S" --for other --metadata keep
say "bad size" 2 refused invalid_arguments 'got "lots"' -- "$S" --for other --max-size lots
say "zero size" 2 refused invalid_arguments 'got "0"' -- "$S" --for other --max-size 0
say "a name that lies" 2 refused invalid_arguments "does not end in .zip" -- "$S" --for other --into "$W/a.tar.gz"
say "a tar name for a zip" 2 refused invalid_arguments "does not end in .tar.gz" -- "$S" --for self --into "$W/a.zip"
say "a folder as the name" 2 refused invalid_arguments "--into needs a file name" -- "$S" --for other --into "$W/somewhere/"
nothing_new "arguments"

# --- help from any position, version, contract ---------------------------------
help_from() {
  local out rc
  out=$("$PACK" "$@" 2>&1 </dev/null); rc=$?
  assert_eq 0 "$rc" "help from '$*': exit"
  has "help from '$*': shows usage" "$out" "USAGE"
}
help_from --help
help_from -h
help_from help
help_from "$S" --help
help_from --for other --help
help_from "$S" --for other -h
help_from contract --help
help_from --into --help
help_from "$S" second --help
help_from --nope --help
HELP=$("$PACK" help 2>&1)
clean "help text" "$HELP"
for word in "hard links" "empty folders" "Finder tags" "download warning" "Links" "unsafe on another system" "never replaces"; do
  has "help says: $word" "$HELP" "$word"
done
"$PACK" "$S" --help --json >"$W/h.json" 2>/dev/null </dev/null; assert_eq 0 "$?" "--help --json: exit"
assert_eq done "$(field status "$W/h.json")" "--help --json: status"
assert_exit 0 "--version" -- "$PACK" --version
"$PACK" contract --json >"$W/c.json" 2>/dev/null </dev/null; assert_eq 0 "$?" "contract --json: exit"
assert_eq pack "$(field details.tool "$W/c.json")" "contract --json: names pack"
OUT=$("$PACK" contract stray 2>&1 >/dev/null </dev/null); RC=$?
assert_eq 2 "$RC" "contract with a stray word: exit"
clean "contract with a stray word" "$OUT"
has "contract with a stray word: words" "$OUT" "contract takes only --json"
nothing_new "help"

# --- the folder ---------------------------------------------------------------
say "a missing folder" 3 error missing_input "no folder at $W/absent." -- "$W/absent" --for other
printf 'x\n' > "$W/plain-file"
say "a file" 3 error missing_input "is a file, and pack takes a folder" -- "$W/plain-file" --for other
mkdir -p "$W/real"; printf 'x\n' > "$W/real/f"; ln -s "$W/real" "$W/link-to-real"
say "a link" 2 refused unsafe_input "is a link, and pack does not follow links" -- "$W/link-to-real" --for other
say "a link with a slash" 2 refused unsafe_input "is a link, and pack does not follow links" -- "$W/link-to-real/" --for other --into "$W/l.zip"
say "the whole disk" 2 refused unsafe_input "/ is the whole disk" -- / --for other --into "$W/disk.zip" --list
say "a system folder" 2 refused unsafe_input "is a system or credential folder" -- /System/Library/Fonts --for other --into "$W/f.zip" --list
mkdir -p "$HOME/.ssh/keys" "$HOME/.aws" "$HOME/Library/Keychains" "$HOME/project/.git/hooks" "$HOME/Documents"
printf 'x\n' > "$HOME/.ssh/keys/known"; printf 'x\n' > "$HOME/.aws/config"; printf 'x\n' > "$HOME/Library/Keychains/data"; printf 'x\n' > "$HOME/project/.git/hooks/h"
for folder in .ssh .ssh/keys .aws Library Library/Keychains project/.git project/.git/hooks; do
  say "credential or system folder $folder" 2 refused unsafe_input "is a system or credential folder" -- "$HOME/$folder" --for other --into "$W/c.zip"
done
say "the home folder" 2 refused unsafe_input "is your home folder" -- "$HOME" --for other --into "$W/h.zip"
say "a folder holding home" 2 refused unsafe_input "is a folder holding your home folder" -- "$W" --for other --into "$W/../etudes-never-made.zip" --list
mkdir -p "$W/Example.app/Contents"; printf 'x\n' > "$W/Example.app/Contents/f"
say "a package" 2 refused unsafe_input "is a package" -- "$W/Example.app" --for other --into "$W/e.zip"
nothing_new "the folder"

# --- things inside the folder: nothing leaves it and nothing is partly done ------
T="$W/has-link"; mkdir -p "$T"; printf 'x\n' > "$T/a"; printf 'outside secret\n' > "$W/outside.txt"; ln -s "$W/outside.txt" "$T/out-link"
say "a link inside" 2 refused unsafe_input "holds a link" -- "$T" --for other --into "$W/i.zip"
hasnt "a link inside: hides the name" "$LAST" "out-link"
STEP=$(printf '%s\n' "$LAST" | sed -n 's/.*To find it: //p')
assert_eq "$T/out-link" "$(eval "$STEP")" "a link inside: the printed step finds it"
rm "$T/out-link"
assert_exit 0 "a link inside: runs once the link is gone" -- "$PACK" "$T" --for other --into "$W/i.zip"
rm -f "${W:?}/i.zip"
T="$W/link-dir"; mkdir -p "$T/d"; printf 'x\n' > "$T/d/f"; ln -s /etc "$T/d/etc"
say "a link to a folder inside" 2 refused unsafe_input "holds a link" -- "$T" --for self --into "$W/i.tar.gz"
T="$W/has-fifo"; mkdir -p "$T"; mkfifo "$T/pipe"
say "a pipe inside" 2 refused unsafe_input "holds a pipe, socket or device" -- "$T" --for other --into "$W/i.zip"
T="$W/has-hard"; mkdir -p "$T"; printf 'x\n' > "$T/a"; ln "$T/a" "$T/b"
say "hard links inside" 2 refused unsafe_input "two names for one file" -- "$T" --for other --into "$W/i.zip"
T="$W/has-hazard"; mkdir -p "$T"; printf 'x\n' > "$T/What?.txt"
say "an unsafe name" 2 refused unsafe_input "not safe to store for every system" -- "$T" --for other --into "$W/i.zip"
hasnt "an unsafe name: hides the name" "$LAST" "What"
T="$W/has-part"; mkdir -p "$T"; printf 'x\n' > "$T/ordinary.txt"; printf 'x\n' > "$T/movie.part"
say "a download" 2 refused download_in_flight '"movie.part" in' -- "$T" --for other --into "$W/i.zip"
T="$W/has-private-part"; mkdir -p "$T"; printf 'x\n' > "$T/passport_scan.pdf.part"
say "a download with a private name" 2 refused download_in_flight "private-looking name" -- "$T" --for other --into "$W/i.zip"
hasnt "a download with a private name: hides it" "$LAST" "passport"
T="$W/split"; mkdir -p "$T/Example.app"; printf 'x\n' > "$T/Example.app/passport_scan.pdf"; printf 'x\n' > "$T/Example.app/part"
say "a package that would be split" 2 refused unsafe_input "goes in whole or not at all" -- "$T" --for other --into "$W/i.zip"
hasnt "a package that would be split: hides the name" "$LAST" "passport"
nothing_new "inside"

# --- unreadable ----------------------------------------------------------------
if [ "$(id -u)" -ne 0 ]; then
  T="$W/has-denied"; mkdir -p "$T"; printf 'x\n' > "$T/a"; printf 'x\n' > "$T/secret-denied"; chmod 000 "$T/secret-denied"
  say "an unreadable file" 3 error permission_denied "cannot read something in $T: permission denied. No archive was made." -- "$T" --for other --into "$W/i.zip"
  hasnt "an unreadable file: hides the name" "$LAST" "secret-denied"
  STEP=$(printf '%s\n' "$LAST" | sed -n 's/.*To list what you cannot read: //p')
  assert_eq "$T/secret-denied" "$(eval "$STEP")" "an unreadable file: the printed step finds it"
  chmod 644 "$T/secret-denied"
  assert_exit 0 "an unreadable file: runs once it can be read" -- "$PACK" "$T" --for other --into "$W/i.zip"
  rm -f "${W:?}/i.zip"
  T="$W/has-denied-folder"; mkdir -p "$T/d"; printf 'x\n' > "$T/d/f"; chmod 000 "$T/d"
  say "an unreadable folder" 3 error permission_denied "cannot read something in $T" -- "$T" --for other --into "$W/i.zip"
  chmod 755 "$T/d"
  chmod 000 "$T"
  say "the folder itself unreadable" 3 error permission_denied "cannot read something in $T" -- "$T" --for other --into "$W/i.zip"
  chmod 755 "$T"
else
  unproven "unreadable items" "mode bits do not deny the superuser"
fi
nothing_new "unreadable"

# --- the destination ------------------------------------------------------------
say "inside the folder" 2 refused invalid_arguments "is inside the folder being packed" -- "$S" --for other --into "$S/in.zip"
mkdir -p "$S/sub"
say "deeper inside the folder" 2 refused invalid_arguments "is inside the folder being packed" -- "$S" --for other --into "$S/sub/in.zip"
ln -s "$S" "$W/alias"
say "inside through a link" 2 refused invalid_arguments "is inside the folder being packed" -- "$S" --for other --into "$W/alias/in.zip"
rm -f "${W:?}/alias"; rmdir "$S/sub"
say "a missing destination folder" 3 error missing_input "Make the folder first: mkdir -p" -- "$S" --for other --into "$W/made/later/o.zip"
STEP=$(printf '%s\n' "$LAST" | sed -n 's/.*Make the folder first: //p')
eval "$STEP"
assert_exit 0 "a missing destination folder: the printed step works" -- "$PACK" "$S" --for other --into "$W/made/later/o.zip"
rm -rf "${W:?}/made"
say "a file in the destination's path" 3 error missing_input "is a file, not a folder" -- "$S" --for other --into "$W/plain-file/o.zip"
taken() {  # taken KIND: the destination already exists as KIND and stays exactly so
  local kind="$1" D="$W/taken-$1.zip" before
  case "$kind" in
    file) printf 'existing bytes\n' > "$D" ;;
    folder) mkdir "$D"; printf 'existing bytes\n' > "$D/keep" ;;
    link) ln -s "$W/plain-file" "$D" ;;
    broken-link) ln -s "$W/nowhere" "$D" ;;
  esac
  before=$(ls -ldi "$D"; [ -d "$D" ] && cat "$D/keep"; [ -f "$D" ] && [ ! -L "$D" ] && cat "$D")
  say "destination is a $kind" 2 refused destination_exists "already exists, and pack never replaces anything" -- "$S" --for other --into "$D"
  assert_eq "$before" "$(ls -ldi "$D"; [ -d "$D" ] && cat "$D/keep"; [ -f "$D" ] && [ ! -L "$D" ] && cat "$D")" "destination is a $kind: untouched"
  rm -rf "${D:?}"
}
taken file
taken folder
taken link
taken broken-link
assert_eq "x" "$(cat "$W/plain-file")" "a linked destination's target was not written"
assert_exit 0 "a destination with an awkward name" -- "$PACK" "$S" --for other --into "$W/it's a name.zip"
rm -f "${W:?}/it's a name.zip"
LONG=$(printf 'n%.0s' $(seq 1 240))
assert_exit 0 "the longest ordinary destination name" -- "$PACK" "$S" --for other --into "$W/$LONG.zip"
rm -f "${W:?}/${LONG:?}.zip"

# --- nothing to include ---------------------------------------------------------
E="$W/empty"; mkdir -p "$E"
say "an empty folder" 1 nothing_to_do nothing_to_do "$E is empty. No archive was made." -- "$E" --for other --into "$W/e.zip"
E2="$W/only-private"; mkdir -p "$E2"; printf 'x\n' > "$E2/passport.pdf"; printf 'x\n' > "$E2/id_rsa"
say "only private items" 1 nothing_to_do nothing_to_do "was left out: 2 items that look personal" -- "$E2" --for other --into "$W/e.zip"
hasnt "only private items: hides the names" "$LAST" "passport"
nothing_new "nothing to include"

# --- what is left out, counted and never named ----------------------------------
T="$W/mixed"; mkdir -p "$T/.ssh" "$T/.git" "$T/node_modules/x"; printf 'x\n' > "$T/keep.txt"; printf 'x\n' > "$T/.env"; printf 'x\n' > "$T/id_rsa"
printf 'x\n' > "$T/server.pem"; printf 'x\n' > "$T/.ssh/config"; printf 'x\n' > "$T/.git/config"; printf 'x\n' > "$T/node_modules/x/f"; printf 'x\n' > "$T/.DS_Store"
OUT=$("$PACK" "$T" --for other --into "$W/m.zip" 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "left out: exit"
clean "left out" "$OUT"
has "left out: counts each kind" "$OUT" "Left out 3 items that look personal, 3 folders of credentials or tool data, 1 Finder leftover."
for name in .env id_rsa server.pem .ssh .git node_modules; do hasnt "left out: hides $name" "$OUT" "$name"; done
assert_eq "mixed/keep.txt" "$(/usr/bin/tar -tf "$W/m.zip")" "left out: only the ordinary file is stored"
rm -f "${W:?}/m.zip"

# --- --list writes nothing, and the size step works ------------------------------
OUT=$("$PACK" "$T" --for other --into "$W/m.zip" --list 2>&1 </dev/null); RC=$?
assert_eq 0 "$RC" "--list: exit"
has "--list: says nothing was written" "$OUT" "Nothing was written"
[ ! -e "$W/m.zip" ] && pass "--list: no archive" || fail "--list: an archive was made"
Z="$W/sized"; mkdir -p "$Z"; dd if=/dev/urandom of="$Z/data" bs=1048576 count=3 status=none
say "over the size limit" 2 refused size_limit_exceeded "over the limit of" -- "$Z" --for other --into "$W/z.zip" --max-size 1M
STEP=$(printf '%s\n' "$LAST" | sed -n 's/.*To allow it: //p')
# shellcheck disable=SC2086
assert_exit 0 "over the size limit: the printed step works" -- "$PACK" "$Z" --for other --into "$W/z.zip" $STEP
rm -f "${W:?}/z.zip"

# The singular.
Y="$W/single"; mkdir -p "$Y"; printf 'x\n' > "$Y/a"; printf 'x\n' > "$Y/passport.pdf"
OUT=$("$PACK" "$Y" --for other --into "$W/y.zip" 2>&1 </dev/null)
clean "singular" "$OUT"
has "singular: one item" "$OUT" "1 item"
has "singular: one left out" "$OUT" "Left out 1 item that looks personal."
rm -f "${W:?}/y.zip"
