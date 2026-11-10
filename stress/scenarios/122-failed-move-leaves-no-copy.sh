#!/usr/bin/env bash
# A move that fails after it made its copy (across volumes) or its second name
# (link then unlink), and before the source was removed, now removes that copy
# or name. When it cannot, the failure names the copy, and sweep and stash no
# longer say "Nothing was moved". Every apply and tag apply that wrote
# journal records now flushes them once on its error returns too.
#
# A nearly full disk image forces a failed cross-volume copy and checks that
# sources keep their bytes and no destination copy survives. Ordinary runs
# also round-trip exact bytes: sweep apply and undo,
# stash and pop, and unpack. Everything is synthetic, in a temp dir, and
# removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
exec </dev/null
require python3 "builds the fixture tree" || exit 0

W=$(workdir) || exit 1
cleanup() {
  detach_registered_mounts
  rm -rf "$W"
  eval "$_stress_state_cleanup"
}
trap cleanup EXIT

make_tree() {
  python3 - "$1" "$2" <<'PY'
import pathlib, sys
d, n = pathlib.Path(sys.argv[1]), int(sys.argv[2])
d.mkdir(parents=True)
for i in range(1, n + 1):
    name = f"Screenshot 2026-0{i % 9 + 1}-{i % 28 + 1:02d} at {i % 12 + 1:02d}.{i % 60:02d}.{i % 60:02d} AM ({i:x}).png"
    (d / name).write_text(f"synthetic screenshot payload {i}\n")
PY
}
home_count() { find "$1" -maxdepth 1 -type f -name 'Screenshot*' | wc -l | tr -d ' '; }
bytes_ok() {
  python3 - "$1" <<'PY'
import pathlib, re, sys
bad = 0
for p in pathlib.Path(sys.argv[1]).glob("Screenshot*"):
    i = int(re.search(r"\(([0-9a-f]+)\)\.png$", p.name).group(1), 16)
    bad += p.read_text() != f"synthetic screenshot payload {i}\n"
print(bad)
PY
}

# sweep apply and undo.
D="$W/swept"; make_tree "$D" 120
OUT=$("$SWEEP" apply "$D" --yes 2>&1); CODE=$?
assert_no_panic "sweep apply" "$OUT"
assert_eq 0 "$CODE" "sweep apply succeeds (exit $CODE)"
assert_eq 0 "$(home_count "$D")" "every file left the folder"
assert_eq 120 "$(find "$D/Screenshots" -type f 2>/dev/null | wc -l | tr -d ' ')" "all 120 files are in Screenshots, with no extra copy"
assert_eq 0 "$(bytes_ok "$D/Screenshots")" "every moved file has its bytes"
OUT=$("$SWEEP" undo "$D" 2>&1); CODE=$?
assert_no_panic "sweep undo" "$OUT"
assert_eq 0 "$CODE" "sweep undo succeeds (exit $CODE)"
assert_eq 120 "$(home_count "$D")" "all 120 files are home after undo"
assert_eq 0 "$(bytes_ok "$D")" "every file has its original bytes"
assert_eq 0 "$(find "$D/Screenshots" -type f 2>/dev/null | wc -l | tr -d ' ')" "nothing is left in Screenshots"

# stash and pop.
S="$W/stashed"; make_tree "$S" 120
OUT=$("$STASH" "$S" 2>&1); CODE=$?
assert_no_panic "stash" "$OUT"
assert_eq 0 "$CODE" "stash clears 120 files (exit $CODE)"
assert_eq 0 "$(home_count "$S")" "no file is left in the folder"
OUT=$("$STASH" pop "$S" 2>&1); CODE=$?
assert_no_panic "stash pop" "$OUT"
assert_eq 0 "$CODE" "pop restores them (exit $CODE)"
assert_eq 120 "$(home_count "$S")" "all 120 files came back"
assert_eq 0 "$(bytes_ok "$S")" "every file came back with its bytes"

# unpack extracts to the exact bytes.
SRC="$W/source"; mkdir -p "$SRC/sub"
printf 'synthetic archive member\n' > "$SRC/member.txt"
printf 'synthetic nested member\n' > "$SRC/sub/nested.txt"
tar -czf "$W/fixture.tar.gz" -C "$SRC" member.txt sub
OUT=$("$UNPACK" "$W/fixture.tar.gz" --into "$W/out" 2>&1); CODE=$?
assert_no_panic "unpack" "$OUT"
assert_eq 0 "$CODE" "unpack extracts the synthetic archive (exit $CODE)"
[ "$(cat "$W/out/member.txt" 2>/dev/null)" = 'synthetic archive member' ] \
  && [ "$(cat "$W/out/sub/nested.txt" 2>/dev/null)" = 'synthetic nested member' ] \
  && pass "unpack wrote both members with their exact bytes" \
  || fail "unpack did not write the exact bytes"

# Force a real cross-volume copy to fail with ENOSPC. The image holds the
# destination; the nested host directory holds three large source files.
if require hdiutil "failed cross-volume move needs a disk image"; then
  M="$W/full-destination"; mkdir -p "$M"
  if create_disk_image "$W/full.dmg" 32m HFS+ FailedCopy && hdiutil attach "$W/full.dmg" -nobrowse -mountpoint "$M" >/dev/null 2>&1; then
    register_mount "$M"
    HOST="$W/cross-source"; mkdir -p "$HOST"
    for i in 1 2 3; do dd if=/dev/zero of="$HOST/report-$i.pdf" bs=1048576 count=16 2>/dev/null; done
    # A directory symlink is not traversed by scan. Mount the image beneath
    # the source root as the existing-scheme route instead.
    PROFILE="$ETUDE_STATE_DIR/sweep-existing-profile.toml"
    mkdir -p "$ETUDE_STATE_DIR"
    # Remount at a real directory in the host root, keeping sources outside it.
    hdiutil detach "$M" >/dev/null 2>&1
    mkdir -p "$HOST/Filed"
    if hdiutil attach "$W/full.dmg" -nobrowse -mountpoint "$HOST/Filed" >/dev/null 2>&1; then
      register_mount "$HOST/Filed"
      printf 'schema_version = 1\n[[routes]]\nextension = "pdf"\nfolder = "Filed"\n' > "$PROFILE"
      dd if=/dev/zero of="$HOST/Filed/filler" bs=1048576 count=128 2>/dev/null || true
      # Free just enough space for metadata and the start of a 16 MiB copy.
      rm -f "$HOST/Filed/filler"
      dd if=/dev/zero of="$HOST/Filed/filler" bs=1048576 count=20 2>/dev/null || true
      SOURCE_HASH=$(shasum -a 256 "$HOST/report-1.pdf" | awk '{print $1}')
      OUT=$("$SWEEP" apply "$HOST" --scheme existing --since 0 --yes 2>&1); RC=$?
      assert_eq 3 "$RC" "cross-volume copy onto a nearly full image fails"
      assert_no_panic "failed cross-volume copy" "$OUT"
      for i in 1 2 3; do
        assert_eq "$SOURCE_HASH" "$(shasum -a 256 "$HOST/report-$i.pdf" 2>/dev/null | awk '{print $1}')" "failed copy preserves original bytes $i"
        [ -f "$HOST/report-$i.pdf" ] && [ "$(stat -f %z "$HOST/report-$i.pdf")" = 16777216 ] \
          && pass "failed copy preserves source $i" || fail "failed copy lost or truncated source $i"
        [ ! -e "$HOST/Filed/report-$i.pdf" ] && pass "failed move leaves no destination copy $i" || fail "failed move left a destination copy $i"
      done
    else
      unproven "failed cross-volume copy" "destination image would not remount"
    fi
  else
    unproven "failed cross-volume copy" "destination image unavailable"
  fi
fi
