#!/usr/bin/env bash
# Run the stress suite and fail only when a scenario fails that we did not
# already know about.
#
# Eleven scenarios fail today by design — they are filed defects kept failing so
# the reproductions stay alive. Gating on zero would make the build permanently
# red, and a red build is one nobody reads.
#
# Gating on a COUNT does not work either, and this script used to. Several
# scenarios race a signal against a two-syscall move, so the same defect reports
# "3 of 50 trials" one run and "8 of 50" the next. The count moved on its own and
# the ratchet cried regression at nothing, then cried "you fixed something" at
# nothing. The set of failing scenarios is stable where the count is not, so the
# set is what is recorded.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

known=$(grep -vE '^[[:space:]]*#|^[[:space:]]*$' stress/baseline.txt | sort -u)

out=$(bash stress/run.sh 2>&1) || true
echo "$out"

actual=$(awk '/^── /{s=$2} /^    FAIL /{print s}' <<<"$out" | sort -u)
passed=$(grep -cE '^    ok ' <<<"$out")
unproven=$(grep -cE '^    unproven ' <<<"$out")

new=$(comm -13 <(echo "$known") <(echo "$actual"))
fixed=$(comm -23 <(echo "$known") <(echo "$actual"))

echo ""
echo "  known failing   $(wc -l <<<"$known" | tr -d ' ') scenarios"
echo "  failing now     $(grep -c . <<<"$actual" | tr -d ' ') scenarios"
echo "  assertions      $passed passed, $unproven unproven"

if [ -n "$fixed" ]; then
  echo ""
  echo "  passing that were expected to fail — re-run before trusting it,"
  echo "  some of these are races that can get lucky:"
  sed 's/^/    /' <<<"$fixed"
fi

if [ "$passed" -eq 0 ]; then
  echo ""
  echo "NOTHING WAS PROVEN — the suite did not really run here."
  exit 2
fi

if [ -n "$new" ]; then
  echo ""
  echo "REGRESSION: these scenarios are failing and are not on the list:"
  sed 's/^/    /' <<<"$new"
  echo ""
  echo "Find the failure above. If it is a genuine new known defect, file it and"
  echo "add the scenario to stress/baseline.txt in the same commit."
  exit 1
fi

echo ""
echo "  no regression."
