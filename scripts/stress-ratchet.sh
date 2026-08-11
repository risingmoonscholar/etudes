#!/usr/bin/env bash
# Run the stress suite and fail only if there are MORE failures than the
# recorded baseline.
#
# Nineteen scenarios fail today by design — they are filed defects, kept
# failing so the reproductions do not rot. Gating on zero would make the build
# permanently red, and a red build is one nobody reads. Gating on "no worse"
# catches a regression the day it appears while letting the known set drain
# through normal work.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

baseline=$(grep -vE '^\s*#|^\s*$' stress/baseline.txt | head -1 | tr -d ' ')
out=$(bash stress/run.sh 2>&1) || true
echo "$out"

actual=$(grep -cE '^    FAIL ' <<<"$out")
passed=$(grep -cE '^    ok ' <<<"$out")
unproven=$(grep -cE '^    unproven ' <<<"$out")

echo ""
echo "  baseline failures: $baseline"
echo "  actual failures:   $actual"
echo "  passed:            $passed"
echo "  unproven:          $unproven"

if [ "$passed" -eq 0 ]; then
  echo "NOTHING WAS PROVEN — the suite did not really run here."
  exit 2
fi
if [ "$actual" -gt "$baseline" ]; then
  echo ""
  echo "REGRESSION: $((actual - baseline)) more failures than the baseline."
  echo "Find the new one above, or lower stress/baseline.txt if you fixed something."
  exit 1
fi
if [ "$actual" -lt "$baseline" ]; then
  echo ""
  echo "Fewer failures than the baseline. Lower stress/baseline.txt to $actual in this commit."
  exit 1
fi
echo "  no regression."
