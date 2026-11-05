#!/usr/bin/env bash
# Pack on small disk images, attached with -nobrowse under this scenario's own
# folder: a disk too small to start, a disk that fills during the write (the run
# is paused, the disk is filled, the run continues), and a read-only disk. Each
# says what happened in plain words, leaves no archive and no unfinished file,
# and the next step works.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir)
cleanup() {
  detach_registered_mounts
  chmod -R u+rwx "${W:?}" 2>/dev/null; rm -rf "${W:?}"
}
trap cleanup EXIT
W=$(cd "$W" && pwd -P)
require hdiutil "disk images are needed for a full disk" || exit 0
export HOME="$W/home" ETUDE_STATE_DIR="$W/state"
mkdir -p "$HOME" "$ETUDE_STATE_DIR"

BANNED='staging|publish|panicked|os error|extractor|malformed'
clean() {
  assert_no_panic "$1" "$2"
  if printf '%s' "$2" | grep -Eqi "$BANNED"; then fail "$1: internal word in: $2"; else pass "$1: plain words"; fi
}
has() { case "$2" in *"$3"*) pass "$1" ;; *) fail "$1: wanted '$3' in: $2" ;; esac; }
field() { python3 -I -c 'import json,sys; v=json.load(open(sys.argv[2]))
for k in sys.argv[1].split("."): v=v[k]
print(v)' "$1" "$2"; }
nothing_left() {  # nothing_left LABEL FOLDER: no archive and no unfinished file
  if [ -z "$(find "$2" -maxdepth 1 \( -name '*.partial' -o -name 'out.*' \) -print -quit)" ]; then
    pass "$1: no archive and no unfinished file"
  else
    fail "$1: something was left: $(ls -A "$2" | tr '\n' ' ')"
  fi
}

# 1. Too small from the start: refused before writing, exit 2.
if create_disk_image "$W/small.dmg" 8m HFS+ etudes-stress-123a; then
  M="$W/small"; mkdir -p "$M"
  if hdiutil attach "$W/small.dmg" -nobrowse -mountpoint "$M" >/dev/null 2>&1; then
    register_mount "$M"
    S="$W/too-big"; mkdir -p "$S"
    dd if=/dev/urandom of="$S/data.bin" bs=1048576 count=12 status=none
    OUT=$("$PACK" "$S" --for other --into "$M/out.zip" 2>&1 </dev/null); RC=$?
    assert_eq 2 "$RC" "too small: exit"
    clean "too small" "$OUT"
    has "too small: says what is needed" "$OUT" "could need up to"
    has "too small: says nothing was made" "$OUT" "No archive was made"
    has "too small: says what to do" "$OUT" "--into FILE"
    "$PACK" "$S" --for other --into "$M/out.zip" --json >"$W/o.json" 2>/dev/null </dev/null
    assert_eq insufficient_space "$(field reason.code "$W/o.json")" "too small: reason"
    assert_eq refused "$(field status "$W/o.json")" "too small: status"
    nothing_left "too small" "$M"
    assert_exit 0 "too small: the printed step works, another disk" -- "$PACK" "$S" --for other --into "$W/elsewhere.zip"
    hdiutil detach "$M" >/dev/null 2>&1
  else
    unproven "too small" "the image did not attach"
  fi
else
  unproven "too small" "the image could not be made"
fi

# 2. Fills during the write. The check before writing passes; then the run is
# paused at a witnessed unfinished file, the disk is filled, and it continues.
fills_proven=0
if create_disk_image "$W/fills.dmg" 200m HFS+ etudes-stress-123b; then
  M="$W/fills"; mkdir -p "$M"
  if hdiutil attach "$W/fills.dmg" -nobrowse -mountpoint "$M" >/dev/null 2>&1; then
    register_mount "$M"
    S="$W/fills-source"; mkdir -p "$S"
    for i in 1 2 3 4 5 6; do dd if=/dev/urandom of="$S/chunk$i.bin" bs=1048576 count=10 status=none; done
    for attempt in 1 2 3 4; do
      "$PACK" "$S" --for other --into "$M/out.zip" --json >"$W/f.out" 2>"$W/f.err" </dev/null &
      PID=$!
      STAGE=""
      for _ in $(seq 1 4000); do
        STAGE=$(find "$M" -maxdepth 1 -name '.out.zip.pack-*.partial' -print -quit)
        if [ -n "$STAGE" ] && [ "$(stat -f %z "$STAGE" 2>/dev/null || echo 0)" -gt 0 ]; then break; fi
        kill -0 "$PID" 2>/dev/null || break
        sleep 0.002
      done
      CHILDREN=$(pgrep -P "$PID" 2>/dev/null || true)
      kill -STOP "$PID" 2>/dev/null
      for c in $CHILDREN; do kill -STOP "$c" 2>/dev/null || true; done
      STATE=$(ps -o state= -p "$PID" 2>/dev/null)
      if [ -z "$STAGE" ] || [[ "$STATE" != *T* ]] || [ -z "$CHILDREN" ]; then
        for c in $CHILDREN; do kill -CONT "$c" 2>/dev/null || true; done
        kill -CONT "$PID" 2>/dev/null; wait "$PID" 2>/dev/null
        rm -f "${M:?}/out.zip"
        echo "    (attempt $attempt: the writer finished first; control reading, not a result)"
        continue
      fi
      dd if=/dev/zero of="$M/filler" bs=1048576 status=none 2>/dev/null
      for c in $CHILDREN; do kill -CONT "$c" 2>/dev/null || true; done
      kill -CONT "$PID" 2>/dev/null
      wait "$PID" 2>/dev/null; RC=$?
      assert_eq 3 "$RC" "the disk fills: exit"
      assert_eq insufficient_space "$(field reason.code "$W/f.out")" "the disk fills: reason"
      assert_eq error "$(field status "$W/f.out")" "the disk fills: status"
      ERR=$(cat "$W/f.err")
      clean "the disk fills" "$ERR"
      has "the disk fills: says so" "$ERR" "the disk ran out of room while writing $M/out.zip"
      has "the disk fills: says nothing was made" "$ERR" "No archive was made"
      rm -f "${M:?}/filler"
      nothing_left "the disk fills" "$M"
      assert_exit 0 "the disk fills: runs after room is freed" -- "$PACK" "$S" --for other --into "$M/out.zip"
      [ -f "$M/out.zip" ] && pass "the disk fills: the second run made the archive" || fail "the disk fills: no archive on the second run"
      fills_proven=1
      break
    done
    [ "$fills_proven" -eq 1 ] || unproven "the disk fills" "the writer finished before it could be paused in 4 attempts"
    hdiutil detach "$M" >/dev/null 2>&1
  else
    unproven "the disk fills" "the image did not attach"
  fi
else
  unproven "the disk fills" "the image could not be made"
fi

# 3. A read-only disk, and a read-only folder on a writable disk.
S="$W/small-source"; mkdir -p "$S"; printf 'bytes\n' > "$S/a.txt"
if [ -f "$W/small.dmg" ]; then
  M="$W/ro"; mkdir -p "$M"
  if hdiutil attach "$W/small.dmg" -readonly -nobrowse -mountpoint "$M" >/dev/null 2>&1; then
    register_mount "$M"
    OUT=$("$PACK" "$S" --for other --into "$M/out.zip" 2>&1 </dev/null); RC=$?
    assert_eq 3 "$RC" "read-only disk: exit"
    clean "read-only disk" "$OUT"
    has "read-only disk: names the folder" "$OUT" "cannot write to $M"
    has "read-only disk: says so" "$OUT" "That disk is read-only"
    "$PACK" "$S" --for other --into "$M/out.zip" --json >"$W/o.json" 2>/dev/null </dev/null
    assert_eq permission_denied "$(field reason.code "$W/o.json")" "read-only disk: reason"
    hdiutil detach "$M" >/dev/null 2>&1
  else
    unproven "read-only disk" "the image did not attach read-only"
  fi
else
  unproven "read-only disk" "the image could not be made"
fi
RO="$W/ro-folder"; mkdir -p "$RO"; chmod 555 "$RO"
OUT=$("$PACK" "$S" --for other --into "$RO/out.zip" 2>&1 </dev/null); RC=$?
assert_eq 3 "$RC" "read-only folder: exit"
clean "read-only folder" "$OUT"
has "read-only folder: names it" "$OUT" "cannot write to $RO: permission denied. No archive was made."
chmod 755 "$RO"
assert_exit 0 "read-only folder: runs once it can be written" -- "$PACK" "$S" --for other --into "$RO/out.zip"
