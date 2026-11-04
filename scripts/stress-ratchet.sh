#!/usr/bin/env bash
# Run the stress suite and check its structured assertion records.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

export STRESS_RESULTS_DIR="${STRESS_RESULTS_DIR:-$PWD/stress/results}"
export STRESS_RUN_ID="${STRESS_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
summary="$STRESS_RESULTS_DIR/$STRESS_RUN_ID/summary.json"
run_status=0
out=$(bash stress/run.sh 2>&1) || run_status=$?
runner_status=$run_status
echo "$out"

if [ -f "$summary" ]; then
  counts=$(python3 - "$summary" <<'PY'
import json, sys
rows = json.load(open(sys.argv[1]))["cases"]
print(sum(row["passed"] for row in rows), sum(row["unproven"] for row in rows))
PY
)
  read -r passed unproven <<<"$counts"
  python3 scripts/check-stress-baseline.py "$summary" --base "${STRESS_BASE_REF:-origin/main}" || run_status=1
else
  passed=0; unproven=0
  [ "$run_status" -ne 0 ] || run_status=1
fi

echo ""
echo "  runner exit     $runner_status"
echo "  assertions      $passed passed, $unproven unproven"

case "$run_status" in
  0) echo "  every executed assertion passed." ;;
  1) echo "  FAILED: a stress assertion or baseline check failed; inspect the evidence above." ;;
  2) echo "  NOTHING WAS PROVEN: required stress coverage is incomplete on this host." ;;
  3) echo "  DID NOT START: another stress run owns the shared resources." ;;
  *) echo "  FAILED: runner exited unexpectedly ($run_status)."; run_status=1 ;;
esac
exit "$run_status"
