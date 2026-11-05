#!/usr/bin/env bash
# Run a command with a private TMPDIR and fail if it leaves anything behind.
#
#   bash scripts/check-temp-leak.sh -- cargo test --all
#   STRESS_TIER=fast bash scripts/check-temp-leak.sh -- bash scripts/stress-ratchet.sh
#   bash scripts/check-temp-leak.sh --self-test
#
# Tests and scenarios make folders in the system temp folder. When one forgets
# to remove its folder, nothing fails: the folders pile up, one run at a time,
# until the disk is full (93,000 of them, over a GB per run, before this
# check). A private TMPDIR is the only honest place to look. In the shared
# one, other programs' files look like ours and ours look like theirs.
#
# Everything the command leaves in the private folder is a failure, except the
# build tool's own scratch names listed in IGNORE below. The folder is removed
# when this script ends, after modes and locked flags are restored.
#
# Exit status: the command's own status when it fails; 1 when it passed but
# left entries; 0 when it passed and left nothing.
set -uo pipefail

# Scratch names the Rust toolchain uses in TMPDIR and does not always remove
# before the command returns. Not this repository's.
IGNORE='^(rustc.*|cargo-.*|\.tmp.*)$'

private=""
cleanup() {
  [ -n "$private" ] && [ -d "$private" ] || return 0
  chflags -R nouchg "$private" 2>/dev/null || true
  chmod -R u+rwX "$private" 2>/dev/null || true
  rm -rf -- "$private"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

run_checked() {
  private=$(mktemp -d "${TMPDIR:-/tmp}/etudes-leak-check-XXXXXX") || return 1
  # An absolute path with no trailing slash, so a name printed below matches
  # the name on disk.
  private=$(cd "$private" && pwd -P)
  local status=0
  TMPDIR="$private" "$@" || status=$?

  local leaked=()
  local name
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    [[ "$name" =~ $IGNORE ]] && continue
    leaked+=("$name")
  done < <(ls -A "$private")

  if [ "$status" -ne 0 ]; then
    echo "check-temp-leak: the command failed (exit $status); leftovers are not judged." >&2
    cleanup
    return "$status"
  fi
  if [ "${#leaked[@]}" -eq 0 ]; then
    echo "check-temp-leak: ok, the run left nothing in its temp folder."
    cleanup
    return 0
  fi
  echo "check-temp-leak: FAILED, the run left ${#leaked[@]} entr$([ "${#leaked[@]}" -eq 1 ] && echo y || echo ies) in the temp folder:" >&2
  local shown=0
  for name in "${leaked[@]}"; do
    shown=$((shown + 1))
    [ "$shown" -le 20 ] || break
    printf '  %s  %s\n' "$(du -sk "$private/$name" 2>/dev/null | awk '{printf "%d KB", $1}')" "$name" >&2
  done
  [ "${#leaked[@]}" -le 20 ] || echo "  ... and $((${#leaked[@]} - 20)) more" >&2
  echo "Remove each folder when its test or scenario ends, pass or fail." >&2
  cleanup
  return 1
}

if [ "${1:-}" = "--self-test" ]; then
  fail=0
  bash "$0" -- bash -c 'mkdir "$TMPDIR/etudes-selftest-leak"' >/dev/null 2>&1
  if [ $? -ne 1 ]; then echo "self-test: a leaking command was NOT caught" >&2; fail=1; fi
  bash "$0" -- bash -c 'd=$(mktemp -d); rmdir "$d"' >/dev/null 2>&1
  if [ $? -ne 0 ]; then echo "self-test: a clean command was rejected" >&2; fail=1; fi
  bash "$0" -- bash -c 'mkdir "$TMPDIR/etudes-selftest-leak"; exit 7' >/dev/null 2>&1
  if [ $? -ne 7 ]; then echo "self-test: a failing command's status was lost" >&2; fail=1; fi
  [ "$fail" -eq 0 ] && echo "check-temp-leak self-test: ok (leak caught, clean run passed, failure status kept)."
  exit "$fail"
fi

[ "${1:-}" = "--" ] && shift
[ "$#" -gt 0 ] || { echo "usage: check-temp-leak.sh [--self-test] -- COMMAND [ARGS...]" >&2; exit 2; }
run_checked "$@"
