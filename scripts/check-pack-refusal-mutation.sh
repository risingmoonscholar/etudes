#!/usr/bin/env bash
# Prove pack's refusal witnesses can fail. A passing check is not enough: three
# planted faults in pack's source must each turn a witness red, for the stated
# reason, and the source must come back byte-identical.
#
#   1. The in-flight refusal returns a different code (unsafe_input).
#   2. That refusal sets no reason at all: pack still exits 2 and prints
#      `"reason":null`, and only the contract check notices.
#   3. A failed post-publication readback reports done instead of incomplete.
#
# Faults 1 and 2 are caught by scripts/check-tool-contracts.py --tool pack.
# Fault 3 cannot be reached by that script: it lists operation_incomplete as
# unreached for pack because no deterministic filesystem interposition makes a
# published archive unreadable. The witness for fault 3 is the unit test
# failed_post_publication_verification_retains_output in crates/pack-cli.
#
# Exit 0 only when every fault was caught; otherwise the uncaught faults are
# named and the exit is non-zero.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

source_file=crates/pack-cli/src/main.rs
backup=$(mktemp "${TMPDIR:-/tmp}/etudes-pack-mutation.XXXXXX")
work=$(mktemp -d "${TMPDIR:-/tmp}/etudes-pack-mutation-work.XXXXXX")
cp "$source_file" "$backup"
UNCAUGHT=()

restore() {
  status=$?
  cp "$backup" "$source_file"
  if ! cmp -s "$backup" "$source_file"; then
    echo "could not restore $source_file; original kept at $backup" >&2
    exit 1
  fi
  echo "source restored and verified identical with cmp"
  rm -f "$backup"
  rm -rf "$work"
  cargo build -p pack-cli >/dev/null 2>&1 || { echo "rebuild after restore failed" >&2; exit 1; }
  if [ "${#UNCAUGHT[@]}" -gt 0 ]; then
    echo "uncaught faults: ${UNCAUGHT[*]}" >&2
    exit 1
  fi
  exit "$status"
}
trap restore EXIT

witness() {
  python3 scripts/check-tool-contracts.py --tool pack --bin target/debug/pack </dev/null
}

# plant PATTERN REPLACEMENT: restore the source, apply one substitution, and
# require that the file really changed. A pattern that matches nothing would
# let a dead witness look alive.
plant() {
  cp "$backup" "$source_file"
  PATTERN="$1" REPLACEMENT="$2" perl -0pi -e 's/$ENV{PATTERN}/$ENV{REPLACEMENT}/' "$source_file"
  if cmp -s "$backup" "$source_file"; then
    echo "planted fault did not apply: the site was not found in $source_file" >&2
    exit 1
  fi
  cargo build -p pack-cli >/dev/null 2>&1 || { echo "pack did not build with the planted fault" >&2; exit 1; }
}

# must_reject NAME EXPECTED_REGEX COMMAND...: the command must fail and must
# say why in the words given. Records the fault as uncaught otherwise.
must_reject() {
  local name="$1" expected="$2" output status
  shift 2
  output=$("$@" 2>&1)
  status=$?
  if [ "$status" -eq 0 ]; then
    echo "$name survived: the witness is vacuous" >&2
    UNCAUGHT+=("$name")
    return
  fi
  if ! printf '%s\n' "$output" | grep -qE "$expected"; then
    echo "$name was rejected for an unrelated reason:" >&2
    printf '%s\n' "$output" >&2
    UNCAUGHT+=("$name")
    return
  fi
  echo "$name was rejected (exit $status):"
  printf '%s\n' "$output" | grep -E "$expected" | head -1 | sed 's/^ */  /'
}

cargo build -p pack-cli >/dev/null 2>&1 || { echo "pack does not build before any fault is planted" >&2; exit 1; }
witness >/dev/null 2>&1 || { echo "the contract check fails before any fault is planted" >&2; exit 1; }
cargo test -p pack-cli failed_post_publication_verification_retains_output >/dev/null 2>&1 \
  || { echo "the post-publication unit test fails before any fault is planted" >&2; exit 1; }

in_flight_site='Code::DownloadInFlight,\n            2,\n            "download is not complete",'

# Fault 1: the right exit status, the wrong code.
plant "$in_flight_site" 'Code::UnsafeInput,
            2,
            "download is not complete",'
must_reject "wrong-code fault" \
  "mismatched: download_in_flight: (contract promises .*unsafe_input|in-flight entry was not named)" \
  witness

# Fault 2: the in-flight refusal records no reason.
plant 'envelope::reason\(Reason::refusal\(self\.code, subject\)\);' \
  'if self.code != Code::DownloadInFlight { envelope::reason(Reason::refusal(self.code, subject)); }'
mkdir -p "$work/state" "$work/tree"
: > "$work/tree/ordinary"
: > "$work/tree/movie.part"
result=$(ETUDE_STATE_DIR="$work/state" \
  ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')" \
  target/debug/pack "$work/tree" --for other --into "$work/out.zip" --json </dev/null 2>/dev/null)
tool_status=$?
if [ "$tool_status" -ne 2 ] || ! printf '%s' "$result" | grep -q '"reason":null'; then
  echo "unset-reason fault did not behave as recorded: exit $tool_status" >&2
  printf '%s\n' "$result" >&2
  UNCAUGHT+=("unset-reason fault")
else
  echo "with no reason set, pack itself still exits $tool_status and prints \"reason\":null"
  must_reject "unset-reason fault" \
    "mismatched: download_in_flight: a refused result has no reason" \
    witness
fi

# Fault 3: a failed post-publication readback is reported as done.
plant 'if verify\(\)\.is_err\(\) \{\n        envelope::status\("incomplete"\);' \
  'if verify().is_err() && false {
        envelope::status("incomplete");'
must_reject "incomplete-says-done fault" \
  "test tests::failed_post_publication_verification_retains_output \.\.\. FAILED" \
  cargo test -p pack-cli failed_post_publication_verification_retains_output

if [ "${#UNCAUGHT[@]}" -eq 0 ]; then
  echo "All three planted pack refusal faults were rejected."
fi
