#!/usr/bin/env bash
# Run every stress scenario against binaries built from this tree.
#
#   stress/run.sh            all scenarios
#   stress/run.sh scale      only scenarios whose name contains "scale"
#
# Exit: 0 all passed · 1 something failed · 2 nothing could be proven here.
#
# Scenarios emulate deployed conditions — a real Desktop mid-project, a synced
# folder, a card dump, an interrupted run. None of it is your data; every tree
# is generated.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

echo "building release binaries"
cargo build --release --quiet || { echo "build failed"; exit 1; }
export BIN="$PWD/target/release"

filter="${1:-}"
TOTAL_P=0; TOTAL_F=0; TOTAL_U=0
ALL_FAIL=(); ALL_UNPROVEN=()

for s in stress/scenarios/*.sh; do
  name=$(basename "$s" .sh)
  [ -n "$filter" ] && [[ "$name" != *"$filter"* ]] && continue
  echo ""
  echo "── $name"
  out=$(SCENARIO="$name" bash "$s" 2>&1)
  echo "$out"
  p=$(grep -c '^    ok ' <<<"$out"); f=$(grep -c '^    FAIL ' <<<"$out"); u=$(grep -c '^    unproven ' <<<"$out")
  # Silence is not success. A scenario that asserted nothing did not run.
  if [ $((p + f + u)) -eq 0 ]; then
    echo "    FAIL     this scenario produced no assertions at all — it did not run"
    f=1
  fi
  TOTAL_P=$((TOTAL_P+p)); TOTAL_F=$((TOTAL_F+f)); TOTAL_U=$((TOTAL_U+u))
  while IFS= read -r l; do [ -n "$l" ] && ALL_FAIL+=("$l"); done < <(grep '^    FAIL ' <<<"$out" | sed "s/^    FAIL *//;s|^|$name: |")
  while IFS= read -r l; do [ -n "$l" ] && ALL_UNPROVEN+=("$l"); done < <(grep '^    unproven ' <<<"$out" | sed "s/^    unproven *//;s|^|$name: |")
done

echo ""
echo "═══════════════════════════════════════════"
printf "  passed   %d\n  failed   %d\n  unproven %d\n" "$TOTAL_P" "$TOTAL_F" "$TOTAL_U"

if [ ${#ALL_FAIL[@]} -gt 0 ]; then
  echo ""; echo "  FAILURES:"; printf '    %s\n' "${ALL_FAIL[@]}"
fi
if [ ${#ALL_UNPROVEN[@]} -gt 0 ]; then
  echo ""
  echo "  NOT PROVEN ON THIS HOST — these are not passes:"
  printf '    %s\n' "${ALL_UNPROVEN[@]}"
fi
echo "═══════════════════════════════════════════"

[ "$TOTAL_F" -gt 0 ] && exit 1
[ "$TOTAL_P" -eq 0 ] && { echo "nothing was proven here"; exit 2; }
exit 0
