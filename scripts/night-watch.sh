#!/usr/bin/env bash
# Attack the tools, then write down what happened in a form a person can read.
#
#   scripts/night-watch.sh            run and print the digest
#   scripts/night-watch.sh --draft    also write a draft post when something is new
#
# This does not post anything anywhere. It writes a draft and stops. A claim
# about this project has to be checkable, and a bot that posts unread numbers
# is the opposite of that.
#
# It reuses scripts/stress-ratchet.sh rather than re-deriving anything: the
# ratchet already knows the baseline and already decides what counts as a
# regression.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

DRAFT=0
[ "${1:-}" = "--draft" ] && DRAFT=1

out=$(bash scripts/stress-ratchet.sh 2>&1); code=$?

baseline=$(grep -vE '^\s*#|^\s*$' stress/baseline.txt | head -1 | tr -d ' ')
actual=$(grep -cE '^    FAIL ' <<<"$out")
passed=$(grep -cE '^    ok ' <<<"$out")
unproven=$(grep -cE '^    unproven ' <<<"$out")
scenarios=$(ls stress/scenarios/*.sh 2>/dev/null | wc -l | tr -d ' ')

echo "night watch: $(date '+%Y-%m-%d')"
echo "  scenarios  $scenarios"
echo "  passed     $passed"
echo "  failed     $actual  (baseline $baseline)"
echo "  unproven   $unproven"

case "$code" in
  0) echo "  verdict    nothing new broke" ;;
  1) echo "  verdict    SOMETHING CHANGED. read the failures above" ;;
  2) echo "  verdict    the suite did not really run; nothing was proven" ;;
esac

# The unproven list is the honest part. Print it every time, not only on failure:
# a hazard that cannot be exercised here is not a hazard that passed.
if [ "$unproven" -gt 0 ]; then
  echo ""
  echo "  not proven on this host: these are not passes:"
  grep -E '^    unproven ' <<<"$out" | sed 's/^    unproven /    /'
fi

# Only worth drafting when the number moved. A nightly post saying "nothing
# happened" is the tease problem in a different costume.
if [ "$DRAFT" = "1" ] && [ "$code" = "1" ]; then
  mkdir -p .night-watch
  f=".night-watch/$(date '+%Y-%m-%d').md"
  {
    echo "draft, not posted. verify every number before this goes anywhere."
    echo
    echo "the stress suite moved: $actual failures against a baseline of $baseline."
    echo
    grep -E '^    FAIL ' <<<"$out" | sed 's/^    FAIL /- /' | head -5
    echo
    echo "reproduce: bash scripts/stress-ratchet.sh"
  } > "$f"
  echo ""
  echo "  draft written: $f"
fi

exit "$code"
