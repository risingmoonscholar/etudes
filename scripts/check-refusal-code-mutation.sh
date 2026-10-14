#!/usr/bin/env bash
# Prove the refusal-code witness can fail. A passing contract check is not
# enough: three planted faults in sweep's source must each turn it red, for the
# expected reason, and the source must come back exactly as it was.
#
#   1. One refusal returns a different code than the contract promises.
#   2. One refusal sets no reason at all.
#   3. An undo that leaves a file behind reports status `incomplete` beside
#      the success reason `done`.
#
# The second fault also records what the shipped tool does in that case. The
# tool does not fail: it exits with its usual refusal status and prints
# `"reason":null`. Only the contract check notices.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

source_file=crates/sweep-cli/src/main.rs
code=project_folder
site='E::RefusedProjectRoot \{ \.\. \} => refusal\(Code::ProjectFolder, None\),'
backup=$(mktemp "${TMPDIR:-/tmp}/etudes-refusal-mutation.XXXXXX")
work=$(mktemp -d "${TMPDIR:-/tmp}/etudes-refusal-mutation-work.XXXXXX")
cp "$source_file" "$backup"

restore() {
  status=$?
  cp "$backup" "$source_file"
  cmp -s "$backup" "$source_file" || { echo "could not restore $source_file; original kept at $backup" >&2; exit 1; }
  rm -f "$backup"
  rm -rf "$work"
  cargo build -p sweep-cli >/dev/null 2>&1 || { echo "rebuild after restore failed" >&2; exit 1; }
  exit "$status"
}
trap restore EXIT

witness() {
  python3 scripts/check-tool-contracts.py --tool sweep --bin target/debug/sweep </dev/null
}

# Replace the one refusal site, rebuild sweep only, and require that the
# source really changed: a pattern that no longer matches would otherwise let
# the witness pass for the wrong reason.
plant() {
  cp "$backup" "$source_file"
  PATTERN="$1" REPLACEMENT="$2" perl -0pi -e 's/$ENV{PATTERN}/$ENV{REPLACEMENT}/' "$source_file"
  if cmp -s "$backup" "$source_file"; then
    echo "planted fault did not apply: the refusal site was not found in $source_file" >&2
    exit 1
  fi
  cargo build -p sweep-cli >/dev/null 2>&1 || { echo "sweep did not build with the planted fault" >&2; exit 1; }
}

# The witness must fail, and must say why in the words given.
must_reject() {
  local name="$1" expected="$2" output status
  set +e
  output=$(witness 2>&1)
  status=$?
  set -e
  if [ "$status" -eq 0 ]; then
    echo "$name survived: the refusal-code witness is vacuous" >&2
    exit 1
  fi
  if ! printf '%s\n' "$output" | grep -qE "$expected"; then
    echo "$name was rejected for an unrelated reason:" >&2
    printf '%s\n' "$output" >&2
    exit 1
  fi
  echo "$name was rejected by the contract check (exit $status):"
  printf '%s\n' "$output" | grep -E "$expected" | head -1 | sed 's/^ */  /'
}

cargo build -p sweep-cli >/dev/null 2>&1
witness >/dev/null 2>&1 || { echo "the contract check fails before any fault is planted" >&2; exit 1; }

# Fault 1: the right exit status, the wrong code.
plant "$site" 'E::RefusedProjectRoot { .. } => refusal(Code::UnknownOption, None),'
must_reject "wrong-code fault" "mismatched: $code: contract promises .*unknown_option"

# Fault 2: the refusal sets no reason.
plant "$site" 'E::RefusedProjectRoot { .. } => ExitCode::from(2),'
mkdir -p "$work/state" "$work/project"
: > "$work/project/Cargo.toml"
set +e
result=$(ETUDE_STATE_DIR="$work/state" \
  ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')" \
  target/debug/sweep "$work/project" --json </dev/null 2>/dev/null)
tool_status=$?
set -e
if [ "$tool_status" -ne 2 ] || ! printf '%s' "$result" | grep -q '"reason":null'; then
  echo "unset-reason fault did not behave as recorded: exit $tool_status" >&2
  printf '%s\n' "$result" >&2
  exit 1
fi
echo "with no reason set, sweep itself still exits $tool_status and prints \"reason\":null"
must_reject "unset-reason fault" "mismatched: $code: a refused result has no reason"

# Fault 3: a restore that left a file behind no longer says so in its reason.
plant 'if !r\.skipped_changed\.is_empty\(\) \{\n        \/\/ Files were left' 'if false {
        // Files were left'
must_reject "incomplete-says-done fault" "mismatched: operation_incomplete: a result with status incomplete carries the success reason done"

echo "All three planted refusal-code faults were rejected."
