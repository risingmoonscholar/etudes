#!/usr/bin/env bash
# Moving the destination's parent folder while unpack extracts takes unpack's
# private folder with it. Whatever unpack then says about what was left must
# be true of the disk: nothing anywhere, or the printed place really holds it
# and the printed step removes it. Both unpack and the system extractor are
# stopped, and seen stopped, before the move.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require zip "the fixture needs zip" || exit 0
require pgrep "stopping the system extractor needs pgrep" || exit 0

W=$(workdir) || exit 1
trap 'chmod -R u+rwx "${W:?}" 2>/dev/null; rm -rf "${W:?}"' EXIT
W=$(cd "${W:?}" && pwd -P) && [ -n "$W" ] || exit 1
# halted PID: the kernel reports the process stopped. sysctl(3) rather than
# ps(1), which a sandbox can refuse to start: CTL_KERN 1, KERN_PROC 14 and
# KERN_PROC_PID 1 (sys/sysctl.h) fill a kinfo_proc whose p_stat byte is at
# offset 36, and SSTOP is 4 (sys/proc.h).
halted() {
  python3 -I - "$1" <<'PY'
import ctypes, sys
libc = ctypes.CDLL(None)
mib = (ctypes.c_int * 4)(1, 14, 1, int(sys.argv[1]))
buf = ctypes.create_string_buffer(1024)
size = ctypes.c_size_t(len(buf))
if libc.sysctl(mib, 4, buf, ctypes.byref(size), None, 0) != 0 or size.value < 37:
    sys.exit(2)
sys.exit(0 if buf.raw[36] == 4 else 1)
PY
}
# stop PID: stop it and wait for the kernel to say so, or resume it.
stop() {
  kill -STOP "$1" 2>/dev/null || return 1
  for _ in $(seq 1 100); do
    halted "$1" && return 0
    kill -0 "$1" 2>/dev/null || return 1
  done
  kill -CONT "$1" 2>/dev/null || true
  return 1
}
sleep 60 & PROBE=$!
if [ "$(uname -s)" != Darwin ] || halted "$PROBE" || ! stop "$PROBE"; then
  kill "$PROBE" 2>/dev/null; wait "$PROBE" 2>/dev/null
  unproven "parent moved mid-extraction" "this host cannot report a stopped process"
  exit 0
fi
kill -CONT "$PROBE" 2>/dev/null; kill "$PROBE" 2>/dev/null; wait "$PROBE" 2>/dev/null
pass "control: a stopped process is seen stopped, and a running one is not"
TOTAL=60
python3 -I - "$W/payload" "$TOTAL" <<'PY'
import os, sys
os.mkdir(sys.argv[1])
for n in range(int(sys.argv[2])):
    with open(os.path.join(sys.argv[1], f"chunk_{n:03}.bin"), "wb") as f:
        f.write(os.urandom(1024 * 1024))
PY
(cd "$W/payload" && zip -q -0 ../input.zip ./*)
rm -rf "$W/payload"
mkdir "$W/p1"

# Private folders for destination "out" anywhere under the work folder.
leftovers() { find "$W" -type d -name '.out.unpack-*.partial' 2>/dev/null | wc -l | tr -d ' '; }
step_after() { printf '%s\n' "$2" | sed -n "s/.*$1//p" | head -1; }

# The detector must see a planted leftover, or a clean result proves nothing.
mkdir -p "$W/control/.out.unpack-00000000000000000000000000000000.partial"
[ "$(leftovers)" = 1 ] && pass "control: the detector finds a planted private folder" \
  || fail "control: the detector missed a planted private folder"
rm -rf "$W/control"

"$UNPACK" "$W/input.zip" --into "$W/p1/out" >"$W/run.out" 2>"$W/run.err" &
PID=$!
STOPPED=()
MOVED=
for _ in $(seq 1 3000); do
  STAGE=$(find "$W/p1" -maxdepth 1 -type d -name '.out.unpack-*.partial' -print -quit 2>/dev/null)
  n=0
  [ -n "$STAGE" ] && n=$(find "$STAGE" -type f -name 'chunk_*.bin' 2>/dev/null | wc -l | tr -d ' ')
  if [ "$n" -gt 0 ] && [ "$n" -lt "$TOTAL" ]; then
    # Stop the supervisor first so it cannot start or reap a writer, then the
    # system extractor, so nothing writes while the parent moves.
    stop "$PID" && STOPPED+=("$PID")
    KIDS=$(pgrep -P "$PID" || true)
    ok=${STOPPED[0]:+1}
    [ -n "$KIDS" ] || ok=
    for kid in $KIDS; do
      if stop "$kid"; then STOPPED+=("$kid"); else ok=; fi
    done
    n=$(find "$STAGE" -type f -name 'chunk_*.bin' 2>/dev/null | wc -l | tr -d ' ')
    if [ -n "$ok" ] && [ "$n" -lt "$TOTAL" ] && mv "$W/p1" "$W/p2"; then
      MOVED=$n
    fi
    break
  fi
  kill -0 "$PID" 2>/dev/null || break
  sleep 0.01
done
# Resume the extractor before the supervisor, which then sees what it did.
for (( i=${#STOPPED[@]}-1; i>=0; i-- )); do kill -CONT "${STOPPED[$i]}" 2>/dev/null || true; done
wait "$PID"; rc=$?

if [ -z "$MOVED" ]; then
  unproven "parent moved mid-extraction" "no confirmed stop of both processes while extraction was partial"
  exit 0
fi
pass "the parent folder moved with $MOVED of $TOTAL files in the private folder, both processes stopped"
ERR=$(cat "$W/run.err")
assert_no_panic "parent moved" "$ERR"
[ "$rc" -ne 0 ] && pass "parent moved: exit is non-zero ($rc)" || fail "parent moved: exit was 0"
[ ! -e "$W/p2/out" ] && [ ! -e "$W/p1/out" ] && pass "parent moved: nothing was published" \
  || fail "parent moved: a destination was published"
[ ! -e "$W/p1" ] && pass "parent moved: the old parent was not made again" \
  || fail "parent moved: $W/p1 was made again"

case "$ERR" in
  *"Nothing was left behind."*)
    assert_eq 0 "$(leftovers)" "parent moved: 'Nothing was left behind' is true of the disk"
    ;;
  *"The partial extraction remains at "*)
    LEFT=$(printf '%s\n' "$ERR" | sed -n 's/.*The partial extraction remains at \(.*\): could not remove it.*/\1/p' | head -1)
    if [ -d "$LEFT" ] && [ -n "$(find "$LEFT" -type f -name 'chunk_*.bin' -print -quit)" ]; then
      pass "parent moved: the printed place holds what was left"
    else
      fail "parent moved: the printed place holds nothing: $LEFT"
    fi
    STEP=$(step_after 'To delete it: ' "$ERR")
    assert_exit 0 "parent moved: the printed step works" -- bash -c "$STEP"
    assert_eq 0 "$(leftovers)" "parent moved: the printed step removed what was left"
    ;;
  *)
    fail "parent moved: says nothing true about what was left: $ERR"
    ;;
esac
