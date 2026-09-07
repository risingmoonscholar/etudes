#!/usr/bin/env bash
# Two related questions about paths sweep cannot fully read:
#
#   1. Path length. Build a path as close to this filesystem's real limit
#      (PATH_MAX, ~1024 bytes on macOS) as mkdir will allow, put a file at
#      the bottom, and confirm sweep finds and moves it without truncating
#      any part of the path. A truncated path is silent data corruption
#      (a move to the wrong place), which is strictly worse than a refusal.
#
#   2. What happens when a directory genuinely cannot be read, because the
#      accumulated path is too long, or (the portable, deterministic way to
#      trigger the exact same code path) because of a permission bit. scan.rs
#      tracks this as `skipped_system`, separately from `skipped_hidden` and
#      `skipped_symlink`. Whether that count (and therefore the fact that
#      part of the folder was invisible to sweep) ever reaches the user is
#      the actual thing under test here.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT

# ======================================================================
# Part 1: near-PATH_MAX: correctness, not refusal
# ======================================================================
D1="$W/pl"; mkdir -p "$D1"
seg() { printf 'd%.0s' $(seq 1 250); printf '%03d' "$1"; }  # 253-byte segment

deepest="$D1"
depth_built=0
for i in 0 1 2 3 4 5 6 7; do
  next="$deepest/$(seg "$i")"
  if mkdir "$next" 2>/dev/null; then
    deepest="$next"
    depth_built=$((depth_built+1))
  else
    break
  fi
done

if [ "$depth_built" -lt 2 ]; then
  unproven "near-PATH_MAX file is discovered and moved intact" "could not build even 2 levels of 253-byte directory names on this filesystem to approach PATH_MAX"
else
  # Fill the remaining PATH_MAX budget with the filename itself, minus a
  # small safety margin (empirically this filesystem's usable ceiling sits
  # a little under the nominal 1024, likely due to how the syscall argument
  # is accounted). Probe downward from a generous starting point instead of
  # assuming an exact number.
  suffix="_pathlentoken.txt"
  placed=0
  for target in 1016 1000 980 950 900 850 800; do
    budget=$((target - ${#deepest} - 1 - ${#suffix}))
    if [ "$budget" -lt 10 ]; then continue; fi
    fname="$(printf 'p%.0s' $(seq 1 "$budget"))${suffix}"
    if : > "$deepest/$fname" 2>/dev/null; then
      placed=1
      break
    fi
  done

  if [ "$placed" -eq 0 ]; then
    unproven "near-PATH_MAX file is discovered and moved intact" "could not create a file near the filesystem's path-length ceiling under $deepest"
  else
    fullpath="$deepest/$fname"
    fullpath_len=${#fullpath}
    printf '    built a %d-byte path %d directories deep\n' "$fullpath_len" "$depth_built" >&2

    # Siblings at the root sharing the same token, so this can be a real,
    # applyable group (a lone deep file with no group is untestable for move
    # correctness: MIN_TOKEN_GROUP is 5).
    for i in 0 1 2 3 4; do : > "$D1/sibling_${i}_pathlentoken.txt"; done

    PLAN_JSON=$("$SWEEP" "$D1" --depth 8 --json 2>&1)
    if grep -qF "$fname" <<<"$PLAN_JSON"; then
      pass "the near-PATH_MAX file ($fullpath_len bytes deep) was discovered by scan and its full filename appears intact in the plan. No truncation"
    else
      fail "the near-PATH_MAX file was not found in the plan at --depth 8. Either it was silently dropped, or its name was altered. scanned tree root: $D1"
    fi

    APPLY_OUT=$("$SWEEP" apply "$D1" --depth 8 --yes 2>&1)
    APPLY_EC=$?
    if [ "$APPLY_EC" = "0" ] && find "$D1" -name "$fname" 2>/dev/null | grep -q .; then
      NEWPATH=$(find "$D1" -name "$fname" 2>/dev/null | head -1)
      pass "the near-PATH_MAX file was moved successfully and its name is unchanged at the new location ($NEWPATH)"
    else
      fail "applying the group containing the near-PATH_MAX file failed or the file cannot be found afterward (apply exit=$APPLY_EC)"
    fi
  fi
fi

# ======================================================================
# Part 2: an unreadable directory is invisible with NO reported signal
# ======================================================================
D2="$W/visibility"; mkdir -p "$D2/locked"
for i in 1 2 3 4 5; do : > "$D2/locked/secret_${i}_lockedtok.txt"; done
for i in 1 2 3 4 5; do : > "$D2/visible_${i}_lockedtok.txt"; done
chmod 000 "$D2/locked"

PLAN2_JSON=$("$SWEEP" "$D2" --depth 2 --json 2>&1)
PLAN2_HUMAN=$("$SWEEP" "$D2" --depth 2 --explain 2>&1)
chmod 755 "$D2/locked"  # restore before any further assertions or cleanup

SCANNED2=$(grep -o '"scanned":[0-9]*' <<<"$PLAN2_JSON" | head -1 | cut -d: -f2)
assert_eq 5 "$SCANNED2" "only the 5 visible files were scanned (the 5 behind the unreadable directory are, as expected, not in the count)"

# The real question: is there ANY signal, anywhere in either output mode,
# that a subdirectory could not be read and its contents are unaccounted
# for? scan.rs increments skipped_system for exactly this case (a read_dir
# failure) but Plan (plan.rs build_with) never copies that field out of
# ScanOutcome, and neither to_json() nor the human render() ever mention it.
# Deliberately NOT matching on the bare word "skip". The JSON schema always
# emits a "skipped" object (for hidden/symlink counts) even when both are
# zero, so a naive substring match on "skip" would false-pass here whether
# or not anything about the permission failure was ever surfaced.
if grep -qi "unreadable\|permission\|could not read\|denied\|eacces\|cannot read\|inaccessible" <<<"$PLAN2_JSON$PLAN2_HUMAN"; then
  pass "sweep disclosed that a directory could not be read"
else
  fail "sweep gave ZERO indication (in --json or in --explain) that \`locked/\` (5 files) could not be read and was silently excluded from the scan. 'Scanned 5 items' reads as complete; it is not. Repro: mkdir locked; put files in it; chmod 000 locked; sweep DIR --explain. Root cause: scan.rs's walk() increments skipped_system on a read_dir() failure (line ~267, also used for refused system-location names), but plan.rs's Plan struct has no skipped_system field at all. build_with() only forwards skipped_hidden and skipped_symlink from ScanOutcome. The count is computed and then thrown away before it ever reaches to_json() or render(). A folder that is partly unreadable (permission error, or a path too long for the OS: same code path) is reported identically to a folder that was fully and successfully scanned. This is silent, not a crash and not a wrong move, but it directly contradicts the tool's own stated design goal (main.rs render(): 'This line must never claim more restraint than actually happened') applied to completeness rather than restraint: the count implies totality it did not have."
fi

AFTER2=$(find "$D2" -type f 2>/dev/null | wc -l | tr -d ' ')
assert_eq 10 "$AFTER2" "nothing was lost or moved. The 5 hidden-by-permission files are still exactly where they were (this is a visibility gap, not a data-safety one)"
