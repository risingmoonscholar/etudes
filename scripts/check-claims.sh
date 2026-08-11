#!/usr/bin/env bash
# Verify the numbers this repo claims about itself are still true.
#
# The README and the demo page both state a test count. A count that drifts is
# the exact failure this project exists to avoid, so it is checked rather than
# maintained by hand. Run it in CI; run it before you publish.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

actual=$(cargo test --all 2>&1 | grep -oE '[0-9]+ passed' | awk '{s+=$1} END {print s}')
status=0

for f in README.md demo/index.html; do
  claimed=$(grep -oE '[0-9]+ tests' "$f" | head -1 | grep -oE '[0-9]+' || true)
  if [ -z "$claimed" ]; then
    echo "no test count claimed in $f — skipping"
    continue
  fi
  if [ "$claimed" != "$actual" ]; then
    echo "FAIL $f claims $claimed tests; the suite runs $actual"
    status=1
  else
    echo "ok   $f claims $claimed tests, and $actual run"
  fi
done

# The readme also states how many stress scenarios fail. That number drifted the
# first time a fix landed, which is exactly the failure this script exists to
# catch, so it is checked against the recorded baseline rather than trusted.
known=$(grep -vcE '^[[:space:]]*#|^[[:space:]]*$' stress/baseline.txt | tr -d ' ')
claimed=$(grep -oE '33 scenarios, [0-9]+ of them failing' README.md | grep -oE '[0-9]+ of them' | grep -oE '[0-9]+' || true)
if [ -n "$claimed" ]; then
  if [ "$claimed" != "$known" ]; then
    echo "FAIL README.md claims $claimed failing scenarios; the baseline lists $known"
    status=1
  else
    echo "ok   README.md claims $claimed failing scenarios, matching the baseline"
  fi
fi

exit $status
