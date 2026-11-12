#!/usr/bin/env bash
# Moving the destination's parent folder while unpack extracts takes unpack's
# private folder with it. Whatever unpack then says about what was left must
# be true of the disk: nothing anywhere, or the printed place really holds it
# and the printed step removes it, or, when the folder cannot be found again,
# a message that says so instead of claiming nothing was left. Three changes
# are made: the parent is moved; the parent is moved and a file takes its
# name; the private folder is moved away and deleted. Each time unpack and the
# system extractor are stopped, and seen stopped, before the change.
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

# run_case LABEL INJECT: extract into p1/out, stop unpack and its extractor
# while the private folder holds part of the archive, make one change, resume
# both, and check that what unpack says was left is true of the disk.
#   moved       the parent folder p1 is moved to p2
#   file        p1 is moved to p2 and a file takes the name p1
#   removed     the private folder is moved away and deleted
run_case() {
  local label="$1" inject="$2" stage n ok kid rc err left step i done_at=""
  local stopped=()
  rm -rf "$W/p1" "$W/p2" "$W/gone"; mkdir "$W/p1"
  "$UNPACK" "$W/input.zip" --into "$W/p1/out" >"$W/run.out" 2>"$W/run.err" &
  local pid=$!
  for _ in $(seq 1 3000); do
    stage=$(find "$W/p1" -maxdepth 1 -type d -name '.out.unpack-*.partial' -print -quit 2>/dev/null)
    n=0
    [ -n "$stage" ] && n=$(find "$stage" -type f -name 'chunk_*.bin' 2>/dev/null | wc -l | tr -d ' ')
    if [ "$n" -gt 0 ] && [ "$n" -lt "$TOTAL" ]; then
      # Stop the supervisor first so it cannot start or reap a writer, then
      # the system extractor, so nothing writes during the change.
      stop "$pid" && stopped+=("$pid")
      local kids; kids=$(pgrep -P "$pid" || true)
      ok=${stopped[0]:+1}
      [ -n "$kids" ] || ok=
      for kid in $kids; do
        if stop "$kid"; then stopped+=("$kid"); else ok=; fi
      done
      n=$(find "$stage" -type f -name 'chunk_*.bin' 2>/dev/null | wc -l | tr -d ' ')
      if [ -n "$ok" ] && [ "$n" -lt "$TOTAL" ]; then
        case "$inject" in
          moved) mv "$W/p1" "$W/p2" && done_at=$n ;;
          file) mv "$W/p1" "$W/p2" && : > "$W/p1" && done_at=$n ;;
          removed) mv "$stage" "$W/gone" && rm -rf "$W/gone" && done_at=$n ;;
        esac
      fi
      break
    fi
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.01
  done
  # Resume the extractor before the supervisor, which then sees what it did.
  for (( i=${#stopped[@]}-1; i>=0; i-- )); do kill -CONT "${stopped[$i]}" 2>/dev/null || true; done
  wait "$pid"; rc=$?

  if [ -z "$done_at" ]; then
    unproven "$label" "no confirmed stop of both processes while extraction was partial"
    return
  fi
  pass "$label: changed with $done_at of $TOTAL files in the private folder, both processes stopped"
  err=$(cat "$W/run.err")
  assert_no_panic "$label" "$err"
  [ "$rc" -ne 0 ] && pass "$label: exit is non-zero ($rc)" || fail "$label: exit was 0"
  [ ! -e "$W/p2/out" ] && [ ! -e "$W/p1/out" ] && pass "$label: nothing was published" \
    || fail "$label: a destination was published"
  case "$inject" in
    moved) [ ! -e "$W/p1" ] && pass "$label: the old parent was not made again" \
      || fail "$label: $W/p1 was made again" ;;
    file) [ -f "$W/p1" ] && [ ! -s "$W/p1" ] && pass "$label: the file that took the old name is untouched" \
      || fail "$label: the file at $W/p1 changed" ;;
  esac

  case "$err" in
    *"Nothing was left behind."*)
      assert_eq 0 "$(leftovers)" "$label: 'Nothing was left behind' is true of the disk"
      ;;
    *"The partial extraction remains at "*)
      left=$(printf '%s\n' "$err" | sed -n 's/.*The partial extraction remains at \(.*\): could not remove it.*/\1/p' | head -1)
      if [ -d "$left" ] && [ -n "$(find "$left" -type f -name 'chunk_*.bin' -print -quit)" ]; then
        pass "$label: the printed place holds what was left"
      else
        fail "$label: the printed place holds nothing: $left"
      fi
      step=$(step_after 'To delete it: ' "$err")
      assert_exit 0 "$label: the printed step works" -- bash -c "$step"
      assert_eq 0 "$(leftovers)" "$label: the printed step removed what was left"
      ;;
    *"The partial extraction is no longer at $stage: it was moved or removed while unpack worked"*)
      [ ! -e "$stage" ] && pass "$label: the private folder is no longer at the printed place" \
        || fail "$label: something is still at $stage"
      case "$err" in
        *"look in the new place for a hidden folder named ${stage##*/} and delete it."*)
          pass "$label: names the hidden folder to look for" ;;
        *) fail "$label: does not name the hidden folder: $err" ;;
      esac
      ;;
    *)
      fail "$label: says nothing true about what was left: $err"
      ;;
  esac
  # A folder that is gone cannot be found again, so unpack must not claim to
  # have removed it.
  if [ "$inject" = removed ]; then
    case "$err" in
      *"Nothing was left behind."*) fail "$label: claims to have removed a folder it could not find" ;;
      *) pass "$label: does not claim to have removed a folder it could not find" ;;
    esac
  fi
}

run_case "parent moved" moved
run_case "parent moved, a file took its name" file
run_case "private folder removed" removed
