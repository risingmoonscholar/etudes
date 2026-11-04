# Stress assertion baseline

`baseline.json` retains every scenario id and a floor for its passing assertion
count. `scripts/stress-ratchet.sh` compares the runner's `summary.json` with
these floors. Printed `ok` lines are not the record.

A missing scenario fails even when its tier is not selected. A scenario listed
in `not_run` has no count comparison. A scenario with unproven assertions also
has no count comparison: a missing host capability can replace many assertions
with one unproven result. The check prints that exemption. Completed scenarios
without unproven results must meet their floor.

The initial floors come from the fast-tier structured records on this branch
and the corrected direct runs of scenarios 73 and 91. The full run recorded
two failed control assertions before those corrections; it is not a passing
full-suite result. Zero denotes
an unmeasured tier, not evidence that the scenario has no assertions. Load and
platform counts have not been measured for this baseline. Raise their floors
from structured results when those tiers can be run. Preserve ids regardless
of tier. Raise an existing floor when a completed run proves more assertions.

Lowering a floor or removing an id is rejected against both HEAD and the
comparison revision. Local runs default to `origin/main`; CI supplies the
pull request base or the preceding revision through `STRESS_BASE_REF`.
Catalog dispositions do not authorize reductions. Changing that policy needs
an explicit repository decision.

Run `python3 -m unittest discover -s scripts -p test_check_stress_baseline.py -v`
to exercise assertion loss, scenario loss, restoration, baseline reductions,
unproven results, and tiers that did not run.
