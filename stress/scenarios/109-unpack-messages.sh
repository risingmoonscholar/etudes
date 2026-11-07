#!/usr/bin/env bash
# Every unpack failure says what failed, whether anything was left, and a next
# step that works, in plain words and without a panic. Every refusal for an
# unsafe archive stays a refusal. Steps that are the point of a fix are followed.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir)
MOUNTS=()
cleanup() {
  for m in ${MOUNTS[@]+"${MOUNTS[@]}"}; do hdiutil detach "$m" -force >/dev/null 2>&1; done
  chmod -R u+rwx "${W:?}" 2>/dev/null; rm -rf "${W:?}"
}
trap cleanup EXIT
W=$(cd "${W:?}" && pwd -P) && [ -n "$W" ] || exit 1
require python3 "the archives here are built with Python" || exit 0
export HOME="$W/home" ETUDE_STATE_DIR="$W/state"
mkdir -p "$HOME" "$ETUDE_STATE_DIR"
A="$W/archives"; mkdir -p "$A"

BANNED='replan|journal seal|malformed|observations|listing failed|extractor|staging|publish|holding locator|holding parent|object identity|member row|dispatched|a surface|This build|os error|panicked|[^0-9]1 entries|[^0-9]1 paths|file\(s\)|path\(s\)|refused invalid'
clean() {  # clean LABEL OUT: no panic and no internal words, whatever the exit
  assert_no_panic "$1" "$2"
  if printf '%s' "$2" | grep -Eqi "$BANNED"; then
    fail "$1: internal word in: $2"
  else
    pass "$1: plain words"
  fi
}
has() { case "$2" in *"$3"*) pass "$1" ;; *) fail "$1: wanted '$3' in: $2" ;; esac; }
hasnt() { case "$2" in *"$3"*) fail "$1: found '$3' in: $2" ;; *) pass "$1" ;; esac; }
field() {  # field 'reason.code' FILE
  python3 -I - "$1" "$2" <<'PY'
import json, sys
value = json.load(open(sys.argv[2]))
for key in sys.argv[1].split("."):
    value = value[key]
print(json.dumps(value) if isinstance(value, bool) else value)
PY
}
# say LABEL EXIT STATUS CODE WORDS -- ARGS...: a text run, then the same with
# --json. Only for commands that change nothing. LAST holds the text.
say() {
  local label="$1" exit="$2" status="$3" code="$4" words="$5"; shift 6
  local out rc
  out=$("$UNPACK" "$@" 2>&1 </dev/null); rc=$?
  clean "$label" "$out"
  assert_eq "$exit" "$rc" "$label: exit"
  has "$label: words" "$out" "$words"
  hasnt "$label: private folder" "$out" ".partial"
  "$UNPACK" "$@" --json >"$W/out.json" 2>"$W/out.err" </dev/null
  assert_eq "$status" "$(field status "$W/out.json")" "$label: status"
  assert_eq "$code" "$(field reason.code "$W/out.json")" "$label: reason"
  clean "$label: json stderr" "$(cat "$W/out.err")"
  LAST="$out"
}
leftovers() {  # nothing beside the archives but what the fixture put there
  if ls -A "$A" | grep -q '\.partial$'; then fail "$1: a private folder was left"; else pass "$1: no private folder"; fi
}
step_after() {  # step_after 'WORDS: ' TEXT: the command printed after those words
  printf '%s\n' "$2" | sed -n "s/.*$1//p" | head -1
}

(cd "$A" && python3 -I - <<'PY'
import gzip, os, tarfile, zipfile
def plain(name):
    info = zipfile.ZipInfo(name); info.external_attr = 0o100644 << 16; return info
def folder(name, mode):
    info = zipfile.ZipInfo(name); info.external_attr = (0o40000 | mode) << 16 | 0x10; return info
with zipfile.ZipFile("good.zip", "w") as z: z.writestr(plain("p.txt"), "hello world")
with zipfile.ZipFile("list.zip", "w") as z: z.writestr(plain("p.txt"), "hello world")
with zipfile.ZipFile("flags.zip", "w") as z: z.writestr(plain("p.txt"), "hello world")
with zipfile.ZipFile("empty.zip", "w"): pass
data = open("empty.zip", "rb").read()
open("counted.zip", "wb").write(data[:8] + b"\x01\x00\x01\x00" + data[12:])
open("trailing.zip", "wb").write(data + b"x")
good = open("good.zip", "rb").read()
open("cut.zip", "wb").write(good[:40])
open("garbage.zip", "wb").write(b"this is not a zip\n")
open("garbage.jar", "wb").write(b"this is not a jar\n")
for name in ("zero.zip", "zero.tar", "zero.tgz", "zero.tar.bz2", "zero.tar.xz", "zero.gz"):
    open(name, "wb").close()
for name in ("garbage.tar", "garbage.tgz", "garbage.tbz", "garbage.txz", "garbage.gz"):
    open(name, "wb").write(b"this is not an archive, and it is longer than one line of text\n" * 40)
with zipfile.ZipFile("crc.zip", "w") as z: z.writestr(plain("payload.txt"), "hello world hello")
damaged = bytearray(open("crc.zip", "rb").read()); damaged[damaged.find(b"hello world")] = ord("j")
open("crc.zip", "wb").write(damaged)
with zipfile.ZipFile("clash.zip", "w") as z:
    z.writestr(plain("x"), "file"); z.writestr(plain("x/y"), "under")
with zipfile.ZipFile("stuck.zip", "w") as z:
    z.writestr(folder("ro/", 0o555), ""); z.writestr(plain("ro/f.txt"), "keep")
    z.writestr(plain("bad.txt"), "hello world hello")
damaged = bytearray(open("stuck.zip", "rb").read()); damaged[damaged.find(b"hello world")] = ord("j")
open("stuck.zip", "wb").write(damaged)
with zipfile.ZipFile("only.zip", "w") as z:
    z.writestr(plain("__MACOSX/._x"), "x"); z.writestr(plain(".DS_Store"), "x")
with zipfile.ZipFile("kindless.zip", "w") as z: z.writestr("no-kind.txt", "x")
gz = gzip.compress(b"hello gzip\n")
open("good.gz", "wb").write(gz); open("cut.gz", "wb").write(gz[:12])
open("empty.tar", "wb").write(b"\0" * 10240)
with tarfile.open("hard.tar", "w") as t:
    i = tarfile.TarInfo("f"); i.size = 0; t.addfile(i)
    i = tarfile.TarInfo("hl"); i.type = tarfile.LNKTYPE; i.linkname = "f"; t.addfile(i)
with tarfile.open("mixed.tar", "w") as t:
    i = tarfile.TarInfo("link"); i.type = tarfile.SYMTYPE; i.linkname = "/etc/passwd"; t.addfile(i)
    i = tarfile.TarInfo("pipe"); i.type = tarfile.FIFOTYPE; t.addfile(i)
    i = tarfile.TarInfo("../out"); i.size = 0; t.addfile(i)
with zipfile.ZipFile("many.zip", "w") as z:
    for number in range(200_001):
        z.writestr(plain(f"{number}"), "")
for name in ("large.zip", "raced.zip"):
    with zipfile.ZipFile(name, "w", zipfile.ZIP_DEFLATED) as z:
        with z.open(plain("zeros"), "w", force_zip64=True) as f:
            for _ in range(256):
                f.write(bytes(1024 * 1024))
for name in ("a.rar", "a.7z", "a.pkg", "nosuffix", "a.zip.part", "a.zip.crdownload", "a.zip.download"):
    open(name, "wb").write(b"anything")
PY
)
(cd "$W" && echo hello > p.txt && zip -q -P pw "$A/secret.zip" p.txt && bzip2 -c p.txt > "$A/bzipped.gz")
NOPE="Nothing was extracted."

# --- damaged, cut short, not this format, no bytes (row 87) ---------------------
for f in cut.zip garbage.zip counted.zip trailing.zip; do
  say "$f" 3 error archive_unreadable "$A/$f could not be read as a ZIP. It may be damaged, incomplete, or not a ZIP. $NOPE If you downloaded it" -- "$A/$f"
done
say garbage.jar 3 error archive_unreadable "could not be read as a ZIP" -- "$A/garbage.jar"
say garbage.tar 3 error archive_unreadable "could not be read as a tar archive" -- "$A/garbage.tar"
say garbage.tgz 3 error archive_unreadable "could not be read as a .tar.gz archive" -- "$A/garbage.tgz"
say garbage.tbz 3 error archive_unreadable "could not be read as a .tar.bz2 archive" -- "$A/garbage.tbz"
say garbage.txz 3 error archive_unreadable "could not be read as a .tar.xz archive" -- "$A/garbage.txz"
for f in zero.zip zero.tar zero.tgz zero.tar.bz2 zero.tar.xz zero.gz; do
  say "$f" 3 error archive_unreadable "$A/$f is 0 bytes" -- "$A/$f"
  has "$f: nothing was left" "$LAST" "$NOPE"
done

# --- an empty archive is not a damaged one (row 88) -----------------------------
say empty.zip 1 nothing_to_do nothing_to_do "the archive is empty." -- "$A/empty.zip"
say empty.tar 1 nothing_to_do nothing_to_do "the archive is empty." -- "$A/empty.tar"
say "empty.zip --list" 1 nothing_to_do nothing_to_do "the archive is empty." -- "$A/empty.zip" --list

# --- a password (row 89) --------------------------------------------------------
say secret.zip 2 refused unsupported_format "$A/secret.zip is password-protected, which unpack does not handle. $NOPE" -- "$A/secret.zip"
STEP=$(step_after 'asks for the password: ' "$LAST")
has "secret.zip: names unzip as the route" "$STEP" "unzip '$A/secret.zip' -d '$A/secret'"
assert_exit 0 "secret.zip: the printed step works with the password" -- bash -c "${STEP/unzip /unzip -P pw }"
assert_eq hello "$(cat "$A/secret/p.txt" 2>/dev/null)" "secret.zip: the printed step extracted it"
rm -rf "$A/secret"
OUT=$("$UNPACK" "$A/secret.zip" --list 2>&1); rc=$?
clean "secret.zip --list" "$OUT"
assert_eq 0 "$rc" "secret.zip --list: exit"
has "secret.zip --list: says so" "$OUT" "password-protected, so unpack would not extract it"
"$UNPACK" "$A/secret.zip" --list --json >"$W/out.json" 2>/dev/null
assert_eq false "$(field details.safe "$W/out.json")" "secret.zip --list: not called safe"
assert_eq true "$(field details.password_protected "$W/out.json")" "secret.zip --list: password flagged"
"$UNPACK" "$A/list.zip" --list --json >"$W/out.json" 2>/dev/null
assert_eq true "$(field details.safe "$W/out.json")" "list.zip --list: an ordinary ZIP stays safe"
assert_eq false "$(field details.password_protected "$W/out.json")" "list.zip --list: no password"
assert_eq true "$(field details.contents_verified "$W/out.json")" "list.zip --list: names were read"

# --- damage only extraction finds (row 90) --------------------------------------
say crc.zip 3 error archive_unreadable "$A/crc.zip could not be fully extracted. It may be damaged, incomplete, or not a ZIP. Nothing was left behind." -- "$A/crc.zip"
has "crc.zip: labelled detail" "$LAST" "Detail from the system tool: payload.txt  bad CRC"
hasnt "crc.zip: no private path" "$LAST" "$A/."
say clash.zip 3 error archive_unreadable "$A/clash.zip could not be fully extracted" -- "$A/clash.zip"
hasnt "clash.zip: no private path" "$LAST" "$A/."
say garbage.gz 3 error archive_unreadable "$A/garbage.gz could not be fully extracted. It may be damaged, incomplete, or not a gzip file. Nothing was left behind." -- "$A/garbage.gz"
has "garbage.gz: labelled detail" "$LAST" "Detail from the system tool: gunzip: garbage.gz: not in gzip format"
hasnt "garbage.gz: no descriptor path" "$LAST" "/dev/fd"
say cut.gz 3 error archive_unreadable "$A/cut.gz could not be fully extracted" -- "$A/cut.gz"
for d in crc clash garbage cut; do
  if [ -e "$A/$d" ]; then fail "$d: a destination appeared"; else pass "$d: no destination"; fi
done
leftovers "damaged archives"

# --- what --list can say about a .gz (row 91) -----------------------------------
say "garbage.gz --list" 3 error archive_unreadable "could not be read as a gzip file" -- "$A/garbage.gz" --list
OUT=$("$UNPACK" "$A/good.gz" --list 2>&1); rc=$?
clean "good.gz --list" "$OUT"
assert_eq 0 "$rc" "good.gz --list: exit"
has "good.gz --list: the expected name" "$OUT" "good.gz would extract to one file named good."
has "good.gz --list: not verified" "$OUT" "not verified"
"$UNPACK" "$A/good.gz" --list --json >"$W/out.json" 2>/dev/null
assert_eq false "$(field details.contents_verified "$W/out.json")" "good.gz --list: json says not verified"
# gunzip reads more than gzip, so a header it reads is not called unreadable.
assert_exit 0 "bzip2 data named .gz: --list" -- "$UNPACK" "$A/bzipped.gz" --list
assert_exit 0 "bzip2 data named .gz: still extracts, as it did" -- "$UNPACK" "$A/bzipped.gz"
rm -rf "$A/bzipped"

# --- unsafe members stay refused and are said plainly (rows 92, 93) -------------
say hard.tar 2 refused unsafe_input "hard link: hl. A hard link can name a file" -- "$A/hard.tar"
say mixed.tar 2 refused unsafe_input "symlink: link. A link inside an archive" -- "$A/mixed.tar"
has "mixed.tar: the device" "$LAST" "device, FIFO or socket: pipe."
has "mixed.tar: the escape" "$LAST" "escapes the target: ../out"
has "mixed.tar: nothing was left" "$LAST" "$NOPE"
assert_eq 1 "$(printf '%s\n' "$LAST" | grep -c -e '--list')" "mixed.tar: the next step is printed once"
STEP=$(step_after 'inside it with: ' "$LAST")
OUT=$(bash -c "${STEP/unpack /\"$UNPACK\" }" 2>&1); rc=$?
assert_eq 0 "$rc" "mixed.tar: the printed step works"
has "mixed.tar: the printed step shows the refusals" "$OUT" "3 unsafe paths would be refused"
if [ -e "$A/mixed" ] || [ -e "$W/out" ]; then fail "mixed.tar: something was written"; else pass "mixed.tar: nothing was written"; fi
say kindless.zip 2 refused unsafe_input "lists \"no-kind.txt\" without saying what kind of item it is" -- "$A/kindless.zip"
STEP=$(step_after 'extract it by hand: ' "$LAST")
assert_exit 0 "kindless.zip: the printed step works" -- bash -c "$STEP"
rm -rf "$A/kindless"

# --- not an archive unpack handles (rows 94, 95) --------------------------------
HANDLED=".tar.gz .tgz .tar.bz2 .tbz .tar.xz .txz .tar .zip .jar .gz."
for s in rar 7z pkg; do
  say ".$s" 2 refused unsupported_format ".$s is not supported. It handles $HANDLED" -- "$A/a.$s"
  assert_eq 1 "$(printf '%s\n' "$LAST" | wc -l | tr -d ' ')" ".$s: one line"
done
say nosuffix 2 refused unsupported_format "$A/nosuffix has no archive suffix. It handles $HANDLED" -- "$A/nosuffix"
for s in part crdownload download; do
  say ".$s" 2 refused download_in_flight "$A/a.zip.$s looks like a download still in progress. Try again when it finishes." -- "$A/a.zip.$s"
  assert_eq later "$(field reason.retry "$W/out.json")" ".$s: retry later"
  hasnt ".$s: does not call the zip unsupported" "$LAST" "not supported"
done

# --- the size limit (row 96) ----------------------------------------------------
say "--max-size 10" 2 refused size_limit_exceeded "stopped at your --max-size limit. It wrote 11 bytes before stopping; the limit is 10 bytes, so it went 1 byte over. Nothing was left behind." -- "$A/good.zip" --max-size 10
hasnt "--max-size 10: the default's words stay out" "$LAST" "half the free space"
hasnt "--max-size 10: no size is guessed" "$LAST" "--max-size 2G"
hasnt "--max-size 10: no soft-limit jargon" "$LAST" "monitored"
STEP=$(step_after 'if you trust this archive: ' "$LAST")
has "--max-size 10: names the archive" "$STEP" "unpack '$A/good.zip' --max-size SIZE"
assert_exit 0 "--max-size 10: the printed step works" -- "$UNPACK" "$A/good.zip" --max-size 1M
if [ -f "$A/good/p.txt" ]; then pass "--max-size: extracted after the step"; else fail "--max-size: the step extracted nothing"; fi
rm -rf "$A/good"
say "--max-size 1 --into" 2 refused size_limit_exceeded "the limit is 1 byte," -- "$A/good.zip" --into "$W/kept" --max-size 1
has "--max-size --into: the step keeps --into" "$LAST" "--into '$W/kept' --max-size SIZE"
leftovers "size limit"

# --- options and their values (rows 98, 99, 100) --------------------------------
OPTIONS='unknown option --frob. It handles --list, --json, --progress-json, --into DIR and --max-size SIZE. Run `unpack help`.'
say "--frob after" 2 refused unknown_option "$OPTIONS" -- "$A/good.zip" --frob
say "--frob first" 2 refused unknown_option "$OPTIONS" -- --frob
say "--frob before the archive" 2 refused unknown_option "$OPTIONS" -- --frob "$A/good.zip"
say "stray word" 2 refused invalid_arguments 'unexpected word "extra". Give one archive and any options.' -- "$A/good.zip" extra
OUT=$("$UNPACK" --list "$A/flags.zip" 2>&1); rc=$?
clean "--list first" "$OUT"; assert_eq 0 "$rc" "--list first: exit"
has "--list first: lists" "$OUT" "p.txt"
if [ -e "$A/flags" ]; then fail "--list first: it extracted"; else pass "--list first: nothing extracted"; fi
assert_exit 0 "--into first" -- "$UNPACK" --into "$W/first" "$A/flags.zip"
if [ -f "$W/first/p.txt" ]; then pass "--into first: went there"; else fail "--into first: not there"; fi
assert_exit 0 "--max-size first" -- "$UNPACK" --max-size 1G "$A/flags.zip"
if [ -f "$A/flags/p.txt" ]; then pass "--max-size first: extracted"; else fail "--max-size first: not extracted"; fi
rm -rf "$A/flags"
"$UNPACK" --json --list "$A/flags.zip" >"$W/out.json" 2>/dev/null
assert_eq done "$(field status "$W/out.json")" "--json first: done"
say "two archives" 2 refused invalid_arguments 'unexpected word' -- --list "$A/flags.zip" "$A/good.zip"
say "--max-size first, too small" 2 refused size_limit_exceeded "your --max-size limit" -- --max-size 1 "$A/flags.zip"
say "unsafe, options first" 2 refused unsafe_input "symlink: link." -- --into "$W/never" "$A/mixed.tar"
if [ -e "$W/never" ]; then fail "unsafe, options first: it wrote"; else pass "unsafe, options first: nothing written"; fi
say "--into with no archive" 2 refused invalid_arguments "--into describes what to do with an archive" -- --into "$W/never"
say "--into value that is a flag" 2 refused invalid_arguments '--into needs a folder, got "--list".' -- "$A/good.zip" --into --list
for bad in 0 0M 0G 1T x -5 --list; do
  say "--max-size $bad" 2 refused invalid_arguments "--max-size needs a whole number above 0, optionally with M or G, got \"$bad\"." -- "$A/good.zip" --max-size "$bad"
done
say '--max-size empty' 2 refused invalid_arguments '--max-size needs a size, such as 500M or 2G.' -- "$A/good.zip" --max-size ""
OUT=$("$UNPACK" "$A/good.zip" --max-size 2>&1); rc=$?
clean '--max-size with no value' "$OUT"; assert_eq 2 "$rc" '--max-size with no value: exit'
has '--max-size with no value: words' "$OUT" '--max-size needs a size, such as 500M or 2G.'
if [ -e "$A/good" ]; then fail "--max-size refusals: something was extracted"; else pass "--max-size refusals: nothing extracted"; fi

# --- paths (rows 101, 102, 103, 104) --------------------------------------------
ln -s "$A/good.zip" "$W/link.zip"
say "archive is a link" 3 error unsafe_input "$W/link.zip is a link, and unpack works only on the real file. Name the file it points to: unpack '$A/good.zip'" -- "$W/link.zip"
STEP=$(step_after 'it points to: ' "$LAST")
assert_exit 0 "archive is a link: the printed step works" -- bash -c "${STEP/unpack /\"$UNPACK\" }"
rm -rf "$A/good"
say "--into under no folder" 3 error missing_input "the folder $W/no/such does not exist, so $W/no/such/out cannot be made there. $NOPE" -- "$A/good.zip" --into "$W/no/such/out"
STEP=$(step_after 'Make the folder first: ' "$LAST")
assert_exit 0 "--into under no folder: the printed step works" -- bash -c "$STEP"
assert_exit 0 "--into under no folder: then it extracts" -- "$UNPACK" "$A/good.zip" --into "$W/no/such/out"
touch "$W/plainfile"
say "--into under a file" 3 error missing_input "part of $W/plainfile is a file" -- "$A/good.zip" --into "$W/plainfile/out"
ln -s "$W/gone" "$W/broken"
say "destination is a broken link" 2 refused destination_exists "$W/broken already exists. Pass --into DIR to choose another." -- "$A/good.zip" --into "$W/broken"
assert_eq "$W/gone" "$(readlink "$W/broken")" "broken link: left alone"
if [ -e "$W/gone" ]; then fail "broken link: followed"; else pass "broken link: not followed"; fi
mkdir "$W/afolder"
say "archive is a folder" 3 error missing_input "not a file: $W/afolder" -- "$W/afolder"

# --- only metadata (row 105) ----------------------------------------------------
OUT=$("$UNPACK" "$A/only.zip" 2>&1); rc=$?
clean only.zip "$OUT"; assert_eq 0 "$rc" "only.zip: exit stays 0"
has "only.zip: says the folder is empty" "$OUT" "The archive held only system metadata, so the folder is empty."
has "only.zip: right plural" "$OUT" "Dropped 2 metadata files"
assert_eq "" "$(ls -A "$A/only")" "only.zip: the folder is empty"
rm -rf "$A/only"
OUT=$("$UNPACK" "$A/flags.zip" 2>&1)
hasnt "an ordinary archive: no such line" "$OUT" "only system metadata"
has "an ordinary archive: singular" "$OUT" "1 entry"
has "an ordinary archive: singular path" "$OUT" "Checked 1 path before"
rm -rf "$A/flags"

# --- a folder that cannot be removed (row 111) ----------------------------------
if [ "$(id -u)" = 0 ]; then
  unproven "leftover folder" "root ignores file modes"
else
  OUT=$("$UNPACK" "$A/stuck.zip" 2>&1); rc=$?
  clean stuck.zip "$OUT"
  assert_eq 3 "$rc" "stuck.zip: exit"
  LEFT=$(ls -d "$A"/.stuck.unpack-*.partial 2>/dev/null | head -1)
  if [ -n "$LEFT" ]; then
    has "stuck.zip: prints the full path" "$OUT" "The partial extraction remains at $LEFT: could not remove it (permission denied)."
    hasnt "stuck.zip: does not claim nothing is left" "$OUT" "Nothing was left behind"
    STEP=$(step_after 'To delete it: ' "$OUT")
    assert_exit 0 "stuck.zip: the printed step works" -- bash -c "$STEP"
    if [ -e "$LEFT" ]; then fail "stuck.zip: the step left the folder"; else pass "stuck.zip: the step removed the folder"; fi
  else
    unproven "leftover folder" "this unzip did not apply the folder mode before failing"
  fi
fi

# --- the download warning mark, and a folder that appears (row 115) -------------
if command -v xattr >/dev/null 2>&1; then
  (cd "$A" && python3 -I - <<'PY'
import io, tarfile
with tarfile.open("locked.tar", "w") as t:
    i = tarfile.TarInfo("locked"); i.mode = 0; i.size = 9; t.addfile(i, io.BytesIO(b"synthetic"))
PY
  )
  xattr -w com.apple.quarantine '0083;00000000;etudes;' "$A/locked.tar"
  say "mark cannot be carried" 2 refused permission_denied "could not carry the download warning mark onto the extracted files: permission denied. Nothing was placed at $A/locked. Nothing was left behind." -- "$A/locked.tar"
  STEP=$(step_after 'extract it by hand: ' "$LAST")
  assert_exit 0 "mark cannot be carried: the printed step works" -- bash -c "$STEP"
  chmod -R u+rwx "$A/locked" 2>/dev/null; rm -rf "$A/locked" "$A/locked.tar"
fi
"$UNPACK" "$A/raced.zip" >"$W/raced.out" 2>&1 &
PID=$!
RACED=
for _ in $(seq 1 3000); do
  if ls -d "$A"/.raced.unpack-*.partial >/dev/null 2>&1; then mkdir "$A/raced"; RACED=1; break; fi
  kill -0 "$PID" 2>/dev/null || break
done
wait "$PID"; rc=$?
if [ -n "$RACED" ] && [ -d "$A/raced" ] && [ -z "$(ls -A "$A/raced")" ] && [ "$rc" != 0 ]; then
  OUT=$(cat "$W/raced.out")
  clean "a folder appeared" "$OUT"
  assert_eq 2 "$rc" "a folder appeared: exit"
  has "a folder appeared: words" "$OUT" "$A/raced appeared while unpack was working, so the extracted files were not put there. Nothing was left behind. Pass --into DIR to choose another."
else
  unproven "a folder appeared" "the extraction finished before the folder was made"
fi
rm -rf "$A/raced"; leftovers "after the race"

# --- too many entries (row 119) -------------------------------------------------
OUT=$("$UNPACK" "$A/many.zip" 2>&1); rc=$?
clean many.zip "$OUT"
assert_eq 2 "$rc" "many.zip: exit"
has "many.zip: words" "$OUT" "holds 200001 entries, and unpack stops at 200000. $NOPE"
has "many.zip: the manual route" "$OUT" "extract it by hand: unzip '$A/many.zip' -d '$A/many'"

# --- stopped by a signal (row 120) ----------------------------------------------
"$UNPACK" "$A/large.zip" --json >"$W/stop.json" 2>"$W/stop.err" &
PID=$!
SENT=
for _ in $(seq 1 3000); do
  if ls -d "$A"/.large.unpack-*.partial >/dev/null 2>&1; then kill -TERM "$PID"; SENT=1; break; fi
  kill -0 "$PID" 2>/dev/null || break
done
wait "$PID"; rc=$?
if [ -n "$SENT" ] && [ "$rc" != 0 ]; then
  clean "stopped" "$(cat "$W/stop.err")"
  assert_eq 2 "$rc" "stopped: exit"
  assert_eq refused "$(field status "$W/stop.json")" "stopped: status"
  assert_eq extraction_stopped "$(field reason.code "$W/stop.json")" "stopped: reason"
  assert_eq input "$(field reason.subject.kind "$W/stop.json")" "stopped: names no undo record"
  has "stopped: words" "$(cat "$W/stop.err")" "stopped after SIGTERM. Nothing was left behind."
else
  unproven "stopped by a signal" "the extraction finished before the signal"
fi
rm -rf "$A/large"; leftovers "after the signal"

# --- full disks (rows 97, 90) ---------------------------------------------------
if require hdiutil "disk images are needed for a full disk"; then
  if create_disk_image "$W/small.dmg" 8m HFS+ etudes-stress-109; then
    M="$W/mnt"; mkdir "$M"
    if hdiutil attach "$W/small.dmg" -nobrowse -mountpoint "$M" >/dev/null 2>&1; then
      MOUNTS+=("$M")
      say "little free space" 2 refused insufficient_space "free on this disk, and unpack needs 256.0 MB free to start. Nothing was written. Free some space, or choose another disk with --into DIR." -- "$A/good.zip" --into "$M/out"
      assert_exit 0 "little free space: --into another disk works" -- "$UNPACK" "$A/good.zip" --into "$W/roomy"
      hdiutil detach "$M" >/dev/null 2>&1; MOUNTS=()
    else
      unproven "little free space" "the image did not attach"
    fi
  else
    unproven "little free space" "the image could not be made"
  fi
  if create_disk_image "$W/fills.dmg" 300m HFS+ etudes-stress-109b; then
    M="$W/mnt2"; mkdir "$M"
    if hdiutil attach "$W/fills.dmg" -nobrowse -mountpoint "$M" >/dev/null 2>&1; then
      MOUNTS+=("$M")
      (cd "$A" && python3 -I - <<'PY'
import zipfile
info = zipfile.ZipInfo("zeros"); info.external_attr = 0o100644 << 16
with zipfile.ZipFile("fills.zip", "w", zipfile.ZIP_DEFLATED) as z:
    with z.open(info, "w", force_zip64=True) as f:
        for _ in range(400):
            f.write(bytes(1024 * 1024))
PY
      )
      say "the disk fills" 3 error insufficient_space "the disk ran out of room while extracting $A/fills.zip. Nothing was left behind. Free some space, or choose another disk with --into DIR." -- "$A/fills.zip" --into "$M/out"
      hasnt "the disk fills: not called damaged" "$LAST" "damaged"
      hdiutil detach "$M" >/dev/null 2>&1; MOUNTS=()
    else
      unproven "the disk fills" "the image did not attach"
    fi
  else
    unproven "the disk fills" "the image could not be made"
  fi
  rm -f "$W/small.dmg" "$W/fills.dmg" "$A/fills.zip"
fi
