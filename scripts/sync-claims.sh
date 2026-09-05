#!/bin/sh
# Write the measured numbers into the files that state them.
#
# check-claims.sh compares two documented counts against reality and fails on
# drift. Adding a test moves the count, so every change that adds one has to
# update README.md and demo/index.html too. Asking a person -- or a model --
# to carry a number they can measure is the wrong shape, and it has been the
# single most expensive mistake in this repo's factory loop: four of eight
# cycles on one unit were spent on it, once by writing 256 when reality was
# 253, once by DELETING the documented count rather than correcting it, twice
# by leaving it stale. Every substantive change in those same cycles was
# correct. The trap is bookkeeping, not judgement.
#
# So this is the mirror of the checker, and the two are held together by
# running the checker afterwards rather than by trusting them to agree. If
# this script's measurement ever diverges from the one that judges it, the
# verification below fails and says so. A sync tool that silently disagreed
# with its checker would be worse than no tool: it would write a number that
# looks authoritative and is wrong.
set -eu
cd "$(dirname "$0")/.."

FILES="README.md demo/index.html"

# Measured exactly as check-claims.sh measures. Test count from the suite's
# own report; scenario count from what git TRACKS, because a reader gets what
# a clone contains and an untracked scenario is not one the readme can claim.
tests=$(cargo test --all 2>&1 | grep -oE '[0-9]+ passed' | awk '{s+=$1} END {print s}')
[ -z "$tests" ] && { echo "FAIL could not count tests; the suite did not report"; exit 1; }
scenarios=$(git ls-files 'stress/scenarios/*.sh' | wc -l | tr -d ' ')

changed=0
for f in $FILES; do
  for pair in "tests:$tests" "scenarios:$scenarios"; do
    word=${pair%%:*}; n=${pair#*:}
    # Refuse rather than guess. A file that no longer states the number has
    # been reworded, and inventing a place to put one would edit prose nobody
    # asked this script to write.
    if ! grep -qE "[0-9]+ $word" "$f"; then
      echo "FAIL $f states no $word count at all; it was reworded. Fix the file or the checker, not this script."
      exit 1
    fi
    before=$(grep -oE "[0-9]+ $word" "$f" | head -1)
    perl -pi -e "s/\b[0-9]+ \Q$word\E\b/$n $word/g" "$f"
    after=$(grep -oE "[0-9]+ $word" "$f" | head -1)
    [ "$before" = "$after" ] || { echo "  $f  $before -> $after"; changed=1; }
  done
done

[ "$changed" -eq 0 ] && echo "  already accurate: $tests tests, $scenarios scenarios"

# The claim this script makes is that the files now match reality. That claim
# is witnessed by the thing that judges it, not asserted here.
echo "  verifying against the checker that judges these files..."
sh scripts/check-claims.sh >/dev/null 2>&1 || {
  echo "FAIL check-claims.sh still fails after syncing. This script's"
  echo "     measurement disagrees with the checker's. Do not trust either"
  echo "     number until they are reconciled."
  sh scripts/check-claims.sh 2>&1 | grep -i fail
  exit 1
}
echo "  check-claims.sh passes: the documented numbers are the measured ones"
