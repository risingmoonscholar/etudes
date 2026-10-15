#!/usr/bin/env bash
# Prove the checks around `sweep propose` can fail. A green test run is not
# enough: four planted faults, one at a time, must each turn a check red, and
# the check that catches each must be the one named here.
#
#   1. A proposal refusal returns another code than the contract promises.
#   2. The check for an item named twice is removed.
#   3. The check that a proposal cites the plan's digest is removed.
#   4. The list of eligible files also takes in the files sweep left alone for
#      other reasons, so a personal-looking file gets an id.
#
# Faults go into a throwaway copy of the tree. The real tree is never edited.
# A fault whose site is not found exactly once stops the script: a pattern that
# silently matched nothing would let a dead check look alive.
#
# Exit 0 only when every fault was caught. Otherwise the exit is non-zero and
# the uncaught faults are named.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
real=$PWD

work=$(mktemp -d "${TMPDIR:-/tmp}/etudes-proposal-mutation.XXXXXX")
# The copy gets fresh file times (tar -m), so a build left by an earlier run
# of this script is never taken for an up-to-date build of this copy.
# Dependencies are rebuilt once and kept between runs; only this scratch
# location is shared, never the tree's own target directory.
export CARGO_TARGET_DIR="${ETUDE_MUTATION_TARGET:-${TMPDIR:-/tmp}/etudes-proposal-mutation-target}"
trap 'rm -rf "$work"' EXIT

copy="$work/tree"
mkdir -p "$copy"
tar -C "$real" --exclude=./target --exclude=./.git --exclude=./stress/results -cf - . | tar -C "$copy" -xmf -
cd "$copy"

proposal=crates/etude-core/src/proposal.rs
reason=crates/etude-cli-support/src/reason.rs

# plant FILE OLD NEW: replace one literal string, and require that it occurs
# exactly once. The file is first restored from the real tree, so each fault
# stands alone.
plant() {
  local file="$1" old="$2" new="$3"
  cp "$real/$file" "$copy/$file"
  OLD="$old" NEW="$new" FILE="$copy/$file" python3 - <<'PY'
import os, sys
path, old, new = os.environ["FILE"], os.environ["OLD"], os.environ["NEW"]
text = open(path, encoding="utf-8").read()
count = text.count(old)
if count != 1:
    sys.exit(f"planted fault did not apply: {old!r} occurs {count} times in {path}, not once")
open(path, "w", encoding="utf-8").write(text.replace(old, new))
PY
}

caught=()
missed=()

# expect_test_failure NAME TEST: the named library test must fail by assertion,
# not by a build error.
expect_test_failure() {
  local name="$1" test="$2" output status
  set +e
  output=$(cargo test -p etude-core --test proposal </dev/null 2>&1)
  status=$?
  set -e
  if [ "$status" -eq 0 ]; then
    echo "$name SURVIVED: every proposal test still passes"
    missed+=("$name")
  elif printf '%s\n' "$output" | grep -qE "^test $test \.\.\. FAILED"; then
    echo "$name was caught by test $test"
    caught+=("$name")
  else
    echo "$name failed for another reason than $test:"
    printf '%s\n' "$output" | tail -15
    missed+=("$name")
  fi
}

# Before any fault: the checks are green on the untouched copy.
cargo test -p etude-core --test proposal </dev/null >"$work/pre.log" 2>&1 \
  || { echo "the proposal tests fail before any fault is planted" >&2; tail -20 "$work/pre.log" >&2; exit 1; }
cargo build -p sweep-cli </dev/null >/dev/null 2>&1 || { echo "sweep did not build before any fault" >&2; exit 1; }
python3 scripts/check-tool-contracts.py --tool sweep --bin "$CARGO_TARGET_DIR/debug/sweep" </dev/null >"$work/pre.log" 2>&1 \
  || { tail -20 "$work/pre.log" >&2; echo "the contract check fails before any fault is planted" >&2; exit 1; }

# Fault 1: the wrong code for a refusal.
plant "$reason" 'Code::UnknownItem => "unknown_item",' 'Code::UnknownItem => "unknown_option",'
cargo build -p sweep-cli </dev/null >/dev/null 2>&1 || { echo "sweep did not build with the wrong-code fault" >&2; exit 1; }
set +e
output=$(python3 scripts/check-tool-contracts.py --tool sweep --bin "$CARGO_TARGET_DIR/debug/sweep" </dev/null 2>&1)
status=$?
set -e
if [ "$status" -ne 0 ] && printf '%s\n' "$output" | grep -qE 'unknown_item'; then
  echo "wrong-code fault was caught by the contract check (exit $status):"
  printf '%s\n' "$output" | grep -E 'unknown_item' | head -1 | sed 's/^ */  /'
  caught+=("wrong-code fault")
else
  echo "wrong-code fault was NOT caught by the contract check (exit $status)"
  printf '%s\n' "$output" | tail -8
  missed+=("wrong-code fault")
fi
cp "$real/$reason" "$copy/$reason"

# Fault 2: an item may be named twice.
plant "$proposal" 'if !seen.insert(index) {' 'if !seen.insert(index) && false {'
expect_test_failure "duplicate-item fault" an_item_named_twice_is_refused_even_when_the_decisions_agree

# Fault 3: a proposal may cite another plan's digest.
plant "$proposal" 'if parsed.digest != plan.digest() {' 'if false && parsed.digest != plan.digest() {'
expect_test_failure "digest-match fault" a_proposal_citing_another_digest_is_refused_stale

# Fault 4: every file sweep left alone becomes eligible, not only the ones no
# group claimed.
plant "$proposal" 'for path in &plan.unclaimed {' 'for (path, _) in &plan.untouched {'
expect_test_failure "eligibility fault" eligible_items_are_the_moves_and_the_unclaimed_and_nothing_else

echo ""
if [ "${#missed[@]}" -ne 0 ]; then
  echo "NOT CAUGHT: ${missed[*]}" >&2
  exit 1
fi
echo "All ${#caught[@]} planted proposal faults were caught."
