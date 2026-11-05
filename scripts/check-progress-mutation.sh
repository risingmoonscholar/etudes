#!/usr/bin/env bash
# Prove the progress tests can fail. Each fault below plants one lie in the
# shared layer or a tool's use of it, and the named test must turn red. A pattern
# that no longer matches stops the script: a fault that did not apply proves
# nothing. Sources are restored afterwards, whatever happens.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

files=(
  crates/etude-cli-support/src/progress.rs
  crates/unpack-cli/src/main.rs
  crates/etude-core/src/apply.rs
)
work=$(mktemp -d "${TMPDIR:-/tmp}/etudes-progress-mutation.XXXXXX")
for file in "${files[@]}"; do cp "$file" "$work/$(basename "$file")"; done
restore() {
  status=$?
  for file in "${files[@]}"; do cp "$work/$(basename "$file")" "$file"; done
  rm -rf "$work"
  exit "$status"
}
trap restore EXIT

# NAME FILE PATTERN REPLACEMENT EXPECTED-OUTPUT -- CARGO-TEST-ARGS
reject() {
  local name="$1" file="$2" pattern="$3" replacement="$4" expected="$5"; shift 6
  cp "$work/$(basename "$file")" "$file"
  PATTERN="$pattern" REPLACEMENT="$replacement" perl -0pi -e 's/$ENV{PATTERN}/$ENV{REPLACEMENT}/' "$file"
  if cmp -s "$work/$(basename "$file")" "$file"; then
    echo "$name: the fault did not apply; the pattern no longer matches $file" >&2
    exit 1
  fi
  local output status
  set +e
  output=$(cargo test "$@" 2>&1)
  status=$?
  set -e
  cp "$work/$(basename "$file")" "$file"
  if [ "$status" -eq 0 ]; then
    echo "$name survived: the progress tests are vacuous" >&2
    exit 1
  fi
  if ! printf '%s\n' "$output" | grep -qE "$expected"; then
    echo "$name failed for an unrelated reason:" >&2
    printf '%s\n' "$output" | tail -30 >&2
    exit 1
  fi
  echo "$name was rejected: $(printf '%s\n' "$output" | grep -E "$expected" | head -1 | cut -c1-120)"
}

reject "counting the whole tree instead of the plan" crates/unpack-cli/src/main.rs \
  '&& planned\.0\.contains\(name\)' '&& !name.is_empty()' 'completed exceeds planned' \
  -- -p unpack-cli --test structured_progress every_format

reject "counting a member that is still being written" crates/unpack-cli/src/main.rs \
  'planned_present\.saturating_sub\(1\)' 'planned_present' 'partly written member' \
  -- -p unpack-cli --test structured_progress a_bare_gzip

reject "a vanished name failing the walk again" crates/unpack-cli/src/main.rs \
  'Err\(error\) if writer_running && error\.kind\(\) == std::io::ErrorKind::NotFound => \{\n\s*continue;\n\s*\}' '' 'No such file' \
  -- -p unpack-cli --test structured_progress a_tar_of_many_files

reject "an extra field in schema 1" crates/etude-cli-support/src/progress.rs \
  '\("journalled", j::num\(journalled\)\),' '("journalled", j::num(journalled)), ("extra", j::num(1)),' 'fields differ from schema 1' \
  -- -p unpack-cli --test structured_progress a_zip_counts

reject "no bound on records" crates/etude-cli-support/src/progress.rs \
  'if event\.completed < self\.next \|\| self\.advances_left == 0 \{\n\s*return;\n\s*\}\n\s*self\.advances_left -= 1;\n\s*self\.next = event\.completed\.saturating_add\(self\.interval\);' '' 'exceeds the bound' \
  -- -p stash-cli --test shared_progress stash_then_pop

reject "done regardless of the outcome" crates/etude-cli-support/src/progress.rs \
  'if crate::envelope::outcome_finished\(code\) \{' 'if true || crate::envelope::outcome_finished(code) {' 'beside result status' \
  -- -p stash-cli --test shared_progress a_changed_item

reject "records on stdout" crates/etude-cli-support/src/progress.rs \
  'record\.push\(.\\n.\);' 'std::println!("{record}"); record.push(0xA as char);' 'progress reached stdout' \
  -- -p stash-cli --test shared_progress restore_is

reject "claiming durability undo never wrote" crates/etude-core/src/apply.rs \
  'event\.completed \+= 1;\n\s*progress\(event\);\n\s*\}\};\n' 'event.completed += 1; event.journalled = event.completed; progress(event); }}; ' 'last record has journalled=5' \
  -- -p sweep-cli --test shared_progress an_undo_that_leaves

echo "ok progress mutation: all planted faults were rejected"
