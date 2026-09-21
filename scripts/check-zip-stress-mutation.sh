#!/usr/bin/env bash
# Prove the ZIP stress witness reaches the pinned system extractor. A normal
# passing scenario is insufficient: this controlled fault must turn it red.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

source_file=crates/unpack-cli/src/main.rs
backup=$(mktemp "${TMPDIR:-/tmp}/etudes-unzip-mutation.XXXXXX")
cp "$source_file" "$backup"

restore() {
  cp "$backup" "$source_file"
  rm -f "$backup"
  cargo build --release -p unpack-cli >/dev/null
}
trap restore EXIT

# The absolute path is intentionally pinned in the product; fault exactly the
# thing users run, not an unrelated PATH lookup.
perl -0pi -e 's#/usr/bin/unzip#/usr/bin/unzip-nightwatch-missing#g' "$source_file"
cargo build --release -p unpack-cli >/dev/null

set +e
BIN="$PWD/target/release" SCENARIO=70-content-blindness-traps \
  bash stress/scenarios/70-content-blindness-traps.sh
fault_status=$?
set -e

if [ "$fault_status" -eq 0 ]; then
  echo "ZIP mutation survived: the stress witness is vacuous" >&2
  exit 1
fi
echo "ZIP mutation was rejected by the stress witness (exit $fault_status)."
