#!/usr/bin/env bash
# Run the stress suite. Every failed assertion is actionable: a file-level
# baseline cannot distinguish a known timing observation from new data loss.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

run_status=0
out=$(bash stress/run.sh 2>&1) || run_status=$?
echo "$out"

passed=$(grep -cE '^    ok ' <<<"$out")
unproven=$(grep -cE '^    unproven ' <<<"$out")

echo ""
echo "  runner exit     $run_status"
echo "  assertions      $passed passed, $unproven unproven"

case "$run_status" in
  0) echo "  every executed assertion passed." ;;
  1) echo "  FAILED: a stress assertion failed; inspect the scenario evidence above." ;;
  2) echo "  NOTHING WAS PROVEN: required stress coverage is incomplete on this host." ;;
  3) echo "  DID NOT START: another stress run owns the shared resources." ;;
  *) echo "  FAILED: runner exited unexpectedly ($run_status)."; run_status=1 ;;
esac
exit "$run_status"
