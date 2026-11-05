#!/usr/bin/env bash
# pack then unpack: the archive is one unpack accepts, and every path and byte
# comes back, for each recipient and format. The tricky shapes are the point: a
# folder whose only item is a subfolder (unpack strips one wrapper), empty
# folders, hidden files, odd but legal names, an executable bit, the deepest tree
# unpack accepts. keep carries Finder tags, attributes and the download warning;
# drop removes them and says so.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir)
cleanup() { chmod -R u+rwx "${W:?}" 2>/dev/null; rm -rf "${W:?}"; }
trap cleanup EXIT
W=$(cd "$W" && pwd -P)
export HOME="$W/home" ETUDE_STATE_DIR="$W/state"
mkdir -p "$HOME" "$ETUDE_STATE_DIR"

# Every path with its kind, mode and content hash. Empty folders count.
manifest() {
  python3 -I - "$1" <<'PY'
import hashlib, os, stat, sys, unicodedata
root = sys.argv[1]
rows = []
for base, dirs, files in os.walk(root):
    for name in dirs + files:
        path = os.path.join(base, name)
        rel = unicodedata.normalize("NFC", os.path.relpath(path, root))
        info = os.lstat(path)
        if stat.S_ISDIR(info.st_mode):
            rows.append(f"dir {rel}")
        else:
            digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
            rows.append(f"file {rel} {oct(info.st_mode & 0o777)} {digest}")
print("\n".join(sorted(rows)))
PY
}
round_trip() {  # round_trip LABEL SOURCE PACK-ARGS...: pack, unpack, compare
  local label="$1" src="$2"; shift 2
  local tag; tag=$(mktemp -d "$W/rt-XXXXXX")
  local archive out="$tag/out"
  case " $* " in
    *" --format tar.gz "*) archive="$tag/a.tar.gz" ;;
    *" --format zip "*|*" --for other "*) archive="$tag/a.zip" ;;
    *) archive="$tag/a.tar.gz" ;;
  esac
  local before; before=$(manifest "$src")
  "$PACK" "$src" --into "$archive" "$@" >"$tag/pack.out" 2>"$tag/pack.err" </dev/null
  assert_eq 0 "$?" "$label: pack"
  "$UNPACK" "$archive" --into "$out" >"$tag/unpack.out" 2>"$tag/unpack.err" </dev/null
  assert_eq 0 "$?" "$label: unpack accepts it"
  assert_eq "$before" "$(manifest "$out")" "$label: every path, mode and byte came back"
  assert_eq "$before" "$(manifest "$src")" "$label: the source did not change"
  RT_OUT="$out"; RT_PACK="$tag/pack.out"
}

# A mixed tree, with both default policies and the other two combinations.
M="$W/mixed"; mkdir -p "$M/notes/deep" "$M/empty/also-empty" "$M/Example.app/Contents"
printf 'report\n' > "$M/report.txt"
printf 'hidden\n' > "$M/.hidden"
printf 'plist\n' > "$M/Example.app/Contents/Info.plist"
dd if=/dev/urandom of="$M/notes/photo.bin" bs=65536 count=4 status=none
: > "$M/notes/empty-file"
printf 'run\n' > "$M/notes/tool.sh"; chmod 755 "$M/notes/tool.sh"
printf 'at\n' > "$M/@starts-with-at"
printf 'space\n' > "$M/ leading space"
printf 'plus\n' > "$M/notes/deep/üñí code.txt"
printf 'nfd\n' > "$M/notes/$(printf 'cafe\xcc\x81').txt"
printf 'emoji\n' > "$M/notes/snow☃.txt"
round_trip "self" "$M" --for self
round_trip "mac" "$M" --for mac
round_trip "other" "$M" --for other
round_trip "self with zip" "$M" --for self --format zip --metadata drop
round_trip "other with tar.gz" "$M" --for other --format tar.gz
LC_ALL=C LANG=C round_trip "a C locale" "$M" --for other

# A .tgz name is the same format.
"$PACK" "$M" --for self --into "$W/named.tgz" >/dev/null 2>&1 </dev/null
assert_eq 0 "$?" "a .tgz name: pack"
"$UNPACK" "$W/named.tgz" --into "$W/named-out" >/dev/null 2>&1 </dev/null
assert_eq 0 "$?" "a .tgz name: unpack accepts it"
assert_eq "$(manifest "$M")" "$(manifest "$W/named-out")" "a .tgz name: everything came back"

# One subfolder and nothing else: unpack strips one wrapper, never a real level.
O="$W/only-sub"; mkdir -p "$O/sub/inner"; printf 'x\n' > "$O/sub/inner/f"
round_trip "only a subfolder" "$O" --for other
[ -f "$RT_OUT/sub/inner/f" ] && pass "only a subfolder: the level is still there" || fail "only a subfolder: a level was lost"
round_trip "only a subfolder, tar.gz" "$O" --for self

# One file; a folder holding only empty folders.
F="$W/one-file"; mkdir -p "$F"; printf 'x\n' > "$F/f"
round_trip "one file" "$F" --for other
E2="$W/only-empty"; mkdir -p "$E2/a/b" "$E2/c"
round_trip "only empty folders" "$E2" --for other

# The default name beside the folder, however the folder was typed.
D="$W/typed"; mkdir -p "$D"; printf 'x\n' > "$D/f"
(cd "$W" && "$PACK" typed/ --for other >/dev/null 2>&1 </dev/null); assert_eq 0 "$?" "a trailing slash: pack"
[ -f "$W/typed.zip" ] && pass "a trailing slash: the archive is beside the folder" || fail "a trailing slash: no typed.zip"
rm -f "${W:?}/typed.zip"
(cd "$D" && "$PACK" . --for other >/dev/null 2>&1 </dev/null); assert_eq 0 "$?" "a dot: pack"
[ -f "$W/typed.zip" ] && pass "a dot: the archive is beside the folder" || fail "a dot: no typed.zip"
assert_eq "f" "$(ls -A "$D")" "a dot: nothing was added to the folder"

# The deepest tree unpack accepts, and one past it, which pack refuses itself.
deep() { local p="$1"; for _ in $(seq 1 "$2"); do p="$p/d"; done; mkdir -p "$p"; printf 'x\n' > "$p/f"; }
T="$W/deep-ok"; mkdir -p "$T"; deep "$T" 30
round_trip "32 levels with the top folder" "$T" --for other
T="$W/deep-no"; mkdir -p "$T"; deep "$T" 32
OUT=$("$PACK" "$T" --for other --into "$W/deep-no.zip" 2>&1 </dev/null); RC=$?
assert_eq 2 "$RC" "too deep: pack refuses before unpack would"
case "$OUT" in *"more than 31 levels down"*) pass "too deep: states the real limit" ;; *) fail "too deep: $OUT" ;; esac
T="$W/deep-edge"; mkdir -p "$T"; deep "$T" 31
assert_exit 2 "one past the limit: refused" -- "$PACK" "$T" --for other --into "$W/deep-edge.zip"
[ ! -e "$W/deep-no.zip" ] && pass "too deep: no archive" || fail "too deep: an archive was made"

# Unpack also refuses setuid files and hard links, so pack refuses them first.
T="$W/setuid"; mkdir -p "$T"; printf 'x\n' > "$T/f"; chmod 4755 "$T/f"
OUT=$("$PACK" "$T" --for self --into "$W/setuid.tar.gz" 2>&1 </dev/null); RC=$?
assert_eq 2 "$RC" "setuid: refused"
case "$OUT" in *"setuid or setgid"*) pass "setuid: says so" ;; *) fail "setuid: $OUT" ;; esac
T="$W/hard-self"; mkdir -p "$T"; printf 'x\n' > "$T/a"; ln "$T/a" "$T/b"
OUT=$("$PACK" "$T" --for self --into "$W/hard-self.tar.gz" 2>&1 </dev/null); RC=$?
assert_eq 2 "$RC" "hard links in tar.gz: refused"
case "$OUT" in *"hard links"*) pass "hard links in tar.gz: says so" ;; *) fail "hard links in tar.gz: $OUT" ;; esac
T="$W/hard-other"; mkdir -p "$T"; printf 'x\n' > "$T/a"; ln "$T/a" "$T/b"
OUT=$("$PACK" "$T" --for other --into "$W/hard-other.zip" 2>&1 </dev/null); RC=$?
assert_eq 2 "$RC" "hard links in zip: refused the same way"
case "$OUT" in *"hard links"*) pass "hard links in zip: says so" ;; *) fail "hard links in zip: $OUT" ;; esac
# A setgid or setuid folder: unpack refuses any such member, so pack does first.
T="$W/setgid-dir"; mkdir -p "$T/shared"; printf 'x\n' > "$T/shared/f"; chmod 2755 "$T/shared"
if [ -n "$(find "$T/shared" -maxdepth 0 -perm -2000)" ]; then
  OUT=$("$PACK" "$T" --for other --into "$W/setgid.zip" 2>&1 </dev/null); RC=$?
  assert_eq 2 "$RC" "a setgid folder: refused"
  case "$OUT" in *"setuid or setgid file or folder"*) pass "a setgid folder: says so" ;; *) fail "a setgid folder: $OUT" ;; esac
  [ ! -e "$W/setgid.zip" ] && pass "a setgid folder: no archive" || fail "a setgid folder: an archive unpack refuses was made"
  STEP=$(printf '%s\n' "$OUT" | sed -n 's/.*To find it: //p')
  assert_eq "$T/shared" "$(eval "$STEP")" "a setgid folder: the printed step finds it"
  chmod 755 "$T/shared"
  round_trip "a setgid folder after the bit is cleared" "$T" --for other
  chmod 2755 "$T"
  round_trip "a setgid top folder, which is not stored" "$T" --for other
else
  unproven "a setgid folder" "this host did not set the bit"
fi
# A second name outside the folder: it goes in as its own copy, and pack says so.
T="$W/hard-out"; mkdir -p "$T"; printf 'x\n' > "$T/a"; ln "$T/a" "$W/second-name"
round_trip "a hard link outside the folder" "$T" --for self
grep -q "1 item has another name that stays out of the archive, and goes in as its own copy." "$RT_PACK" \
  && pass "a hard link outside the folder: says so" || fail "a hard link outside the folder: $(cat "$RT_PACK")"

# An attribute pack does not count must not look like the person's size limit.
T="$W/big-attr"; mkdir -p "$T"; printf 'x\n' > "$T/a"
python3 -I -c 'import os, subprocess, sys
subprocess.run(["/usr/bin/xattr", "-w", "user.blob", os.urandom(300 * 1024).hex(), sys.argv[1]], check=True)' "$T/a"
round_trip "a large uncounted attribute" "$T" --for mac
assert_exit 0 "a large uncounted attribute: a large limit also works" -- "$PACK" "$T" --for mac --max-size 100G --into "$W/big-attr.tar.gz"
rm -f "${W:?}/big-attr.tar.gz"
OUT=$("$PACK" "$T" --for mac --max-size 200K --into "$W/big-attr.tar.gz" 2>&1 </dev/null); RC=$?
assert_eq 2 "$RC" "a large attribute over a real limit: refused"
case "$OUT" in *"passed the limit of"*) pass "a large attribute over a real limit: says so" ;; *) fail "a large attribute over a real limit: $OUT" ;; esac
STEP=$(printf '%s\n' "$OUT" | sed -n 's/.*To allow more: //p')
# shellcheck disable=SC2086
assert_exit 0 "a large attribute over a real limit: the printed step works" -- "$PACK" "$T" --for mac --into "$W/big-attr.tar.gz" $STEP

# Metadata. keep carries it; drop removes it, counts it, and warns.
require xattr "attributes need xattr" || exit 0
T="$W/meta"; mkdir -p "$T"; printf 'a\n' > "$T/a"; printf 'b\n' > "$T/b"; printf 'c\n' > "$T/c"
TAG_PLIST='<?xml version="1.0"?><plist version="1.0"><array><string>Red</string></array></plist>'
xattr -w com.apple.quarantine '0081;00000000;Test;' "$T/a"
xattr -w com.apple.metadata:_kMDItemUserTags "$TAG_PLIST" "$T/b"
xattr -w user.note 'kept' "$T/c"
xattr -w com.apple.ResourceFork 'fork' "$T/c"
round_trip "metadata, keep" "$T" --for mac
COUNTS=$("$PACK" "$T" --for mac --into "$W/counts.tar.gz" --list --json 2>/dev/null </dev/null | python3 -I -c 'import json,sys; d=json.load(sys.stdin)["details"]; print(d["items_with_download_warning"], d["items_with_finder_tags"], d["items_with_fork_or_finder_info"])')
assert_eq "1 1 1" "$COUNTS" "metadata: each kind is counted"
grep -q "Mac metadata carried: Finder tags on 1 item, the download warning on 1 item, resource forks or Finder info on 1 item, any other attributes." "$RT_PACK" \
  && pass "metadata, keep: says what it carried" || fail "metadata, keep: $(cat "$RT_PACK")"
xattr -p com.apple.quarantine "$RT_OUT/a" >/dev/null 2>&1 && pass "metadata, keep: the download warning came back" || fail "metadata, keep: the download warning was lost"
xattr -p com.apple.metadata:_kMDItemUserTags "$RT_OUT/b" 2>/dev/null | grep -q Red && pass "metadata, keep: the Finder tag came back" || fail "metadata, keep: the Finder tag was lost"
assert_eq kept "$(xattr -p user.note "$RT_OUT/c" 2>/dev/null)" "metadata, keep: the other attribute came back"
assert_eq fork "$(xattr -p com.apple.ResourceFork "$RT_OUT/c" 2>/dev/null)" "metadata, keep: the resource fork came back"
round_trip "metadata, drop" "$T" --for other
grep -q "Mac metadata removed: Finder tags on 1 item, the download warning on 1 item, resource forks or Finder info on 1 item, any other attributes." "$RT_PACK" \
  && pass "metadata, drop: says what it removed" || fail "metadata, drop: $(cat "$RT_PACK")"
grep -q "will not warn when it is extracted" "$RT_PACK" && pass "metadata, drop: warns about the download warning" || fail "metadata, drop: no warning"
xattr "$RT_OUT/a" "$RT_OUT/b" "$RT_OUT/c" 2>/dev/null | grep -Eq 'quarantine|UserTags|user.note|ResourceFork' \
  && fail "metadata, drop: an attribute survived" || pass "metadata, drop: every attribute is gone"

# Finder litter. A stored ._ file is left out in both modes; drop leaves out the
# others too. A package is not refused over litter.
T="$W/litter"; mkdir -p "$T/x.app"; printf 'a\n' > "$T/a"; printf 'shadow\n' > "$T/._a"; printf 'finder\n' > "$T/.DS_Store"
printf 'thumb\n' > "$T/Thumbs.db"; printf 'part\n' > "$T/x.app/part"; printf 'finder\n' > "$T/x.app/.DS_Store"; xattr -w user.note kept "$T/a"
"$PACK" "$T" --for self --into "$W/litter.tar.gz" >"$W/l.out" 2>&1 </dev/null
assert_eq 0 "$?" "litter, keep: a stored ._ file beside its own attributes does not stop it"
"$PACK" "$T" --for other --into "$W/litter.zip" >"$W/l.out" 2>&1 </dev/null
assert_eq 0 "$?" "litter, drop: pack"
grep -q "Left out 4 Finder leftovers." "$W/l.out" && pass "litter, drop: counted" || fail "litter, drop: $(cat "$W/l.out")"
/usr/bin/tar -tf "$W/litter.zip" | grep -Eq '\.DS_Store|Thumbs\.db|/\._' && fail "litter, drop: litter was stored" || pass "litter, drop: none stored"
/usr/bin/tar -tf "$W/litter.zip" | grep -q 'x.app/part' && pass "litter, drop: the package is whole" || fail "litter, drop: the package lost a part"
