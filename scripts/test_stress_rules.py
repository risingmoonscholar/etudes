"""The stress suite's own rules: what makes a scenario a good test case.

These tests are written before the fixes. They fail until the suite meets them.

1. The floor of passed assertions is checked on every run, also when a
   scenario reports unproven results, and no floor is 0.
2. Each scenario's unproven count is pinned in stress/unproven.json. A run
   with more or fewer unproven results than the pin fails.
3. A scenario that stops early says so.
4. Every scenario follows the source rules in scripts/check-stress-taste.py.
5. Every planted bug in stress/mustfire is valid, and every scenario that the
   audit found weak or bad has at least one planted bug or source rule.
6. A planted-bug shim runs the real binary unchanged for calls it does not target.
"""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


baseline = load("baseline", HERE / "check-stress-baseline.py")
taste = load("taste", HERE / "check-stress-taste.py")
mustfire = load("mustfire", ROOT / "stress/mustfire/run.py")

# Scenarios the 0.5.5 audit found weak or bad. Each needs a planted bug in
# stress/mustfire/cases.json, a source rule in check-stress-taste.py, or both.
AUDITED = {
    "15-document-scopes", "15-real-project-layouts", "20-apple-metadata",
    "20-unicode-nfc-nfd-collision", "25-grace-window", "30-concurrent-applies",
    "30-graceful-signal-mid-apply", "30-interrupted-stash-pop", "30-journal-supplied-key",
    "30-journal-truncation", "30-same-root-contention", "30-sigkill-mid-apply",
    "32-stash-explicit-selection", "40-06-permission-change-mid-apply",
    "50-scale-large-and-zero-byte", "50-scale-path-length-and-visibility",
    "50-scale-pathological-names", "60-case-sensitive-collision",
    "60-cross-device-copy-and-mtime", "60-cross-volume-exdev", "60-full-volume",
    "60-readonly-volume", "60-rlimit-nofile", "60-undo-progress-lost-on-volume-hazard",
    "70-content-blindness-traps", "80-interrupted-extraction", "85-scenario-exit-status",
    "87-stash-finder-tags", "89-tagged-cross-volume", "98-unpack-parallel-zip",
    "102-sweep-existing", "105-permission-denied-messages", "106-failure-steps-are-true",
    "107-sweep-and-plan-messages", "109-unpack-messages", "110-final-message-consistency",
    "112-locked-items", "113-shared-progress", "115-non-utf8-arguments",
    "122-failed-move-leaves-no-copy", "128-moves-from-checked-source-folders",
}
# Weak scenarios whose gap no planted bug or source rule can show. The fix is
# checked by the reviewing seat against the audit text in
# stress/mustfire/README.md.
REVIEW_ONLY = {"15-real-project-layouts", "70-content-blindness-traps", "106-failure-steps-are-true",
               "122-failed-move-leaves-no-copy"}
# Gaps a source rule checks (the rule names are in check-stress-taste.py).
RULED = {
    "20-apple-metadata", "20-unicode-nfc-nfd-collision", "25-grace-window",
    "30-concurrent-applies", "30-graceful-signal-mid-apply", "30-interrupted-stash-pop",
    "30-journal-supplied-key", "30-same-root-contention", "30-sigkill-mid-apply",
    "32-stash-explicit-selection", "40-06-permission-change-mid-apply",
    "60-full-volume", "60-rlimit-nofile", "80-interrupted-extraction",
    "85-scenario-exit-status", "109-unpack-messages", "113-shared-progress",
}


def summary(passed, unproven):
    return {"cases": [{"id": "case", "passed": passed, "unproven": unproven}], "not_run": []}


class FloorAndPinTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        root = Path(self.dir.name)
        (root / "stress/scenarios").mkdir(parents=True)
        (root / "stress/scenarios/case.sh").write_text("pass one\n")
        self.patch = patch.object(baseline, "ROOT", root)
        self.patch.start()
        self.catalog = [{"id": "case"}]

    def tearDown(self):
        self.patch.stop()
        self.dir.cleanup()

    def test_floor_is_checked_when_a_scenario_reports_unproven(self):
        errors = baseline.check({"case": 5}, self.catalog, summary(3, 1), pins={"case": 1})
        self.assertTrue(any("below baseline" in e for e in errors), errors)

    def test_more_unproven_than_pinned_fails(self):
        errors = baseline.check({"case": 3}, self.catalog, summary(3, 2), pins={"case": 1})
        self.assertTrue(any("unproven" in e for e in errors), errors)

    def test_fewer_unproven_than_pinned_fails_until_the_pin_is_lowered(self):
        errors = baseline.check({"case": 3}, self.catalog, summary(3, 0), pins={"case": 1})
        self.assertTrue(any("unproven" in e for e in errors), errors)

    def test_an_unpinned_unproven_fails(self):
        errors = baseline.check({"case": 3}, self.catalog, summary(3, 1), pins={})
        self.assertTrue(any("unproven" in e for e in errors), errors)

    def test_pinned_count_and_floor_met_passes(self):
        self.assertEqual([], baseline.check({"case": 3}, self.catalog, summary(3, 1), pins={"case": 1}))

    def test_a_floor_of_zero_fails(self):
        errors = baseline.check({"case": 0}, self.catalog, summary(3, 0), pins={})
        self.assertTrue(any("floor" in e for e in errors), errors)


class CommittedDataTests(unittest.TestCase):
    def test_every_floor_is_at_least_one(self):
        floors = json.loads((ROOT / "stress/baseline.json").read_text())
        zero = sorted(k for k, v in floors.items() if v < 1)
        self.assertEqual([], zero, "raise these floors from a measured full run")

    def test_pins_name_catalogued_scenarios(self):
        pins = json.loads((ROOT / "stress/unproven.json").read_text())
        ids = {row["id"] for row in json.loads((ROOT / "stress/catalog.json").read_text())}
        self.assertEqual(set(), set(pins) - ids)
        self.assertTrue(all(type(v) is int and v >= 1 for v in pins.values()),
                        "list only scenarios with unproven results; a pin is a positive count")


class HarnessTests(unittest.TestCase):
    def test_a_scenario_that_stops_early_says_so(self):
        with tempfile.TemporaryDirectory() as d:
            script = Path(d) / "probe.sh"
            script.write_text(f'source "{ROOT}/stress/lib.sh"\npass "one"\nexit 3\n')
            env = dict(os.environ, BIN=d, SCENARIO="probe")
            env.pop("ETUDE_JOURNAL_KEY", None)
            p = subprocess.run(["bash", str(script)], cwd=d, env=env, capture_output=True,
                               text=True, timeout=60, stdin=subprocess.DEVNULL)
        self.assertIn("scenario exited 3 before finishing", p.stdout + p.stderr)
        self.assertNotEqual(0, p.returncode)

    def test_every_scenario_follows_the_source_rules(self):
        p = subprocess.run([sys.executable, str(HERE / "check-stress-taste.py")],
                           capture_output=True, text=True)
        self.assertEqual(0, p.returncode, p.stdout[-4000:])


class PlantedBugTests(unittest.TestCase):
    def test_cases_are_valid(self):
        _, errors = mustfire.load_cases()
        self.assertEqual([], errors)

    def test_every_audited_scenario_is_covered(self):
        cases, _ = mustfire.load_cases()
        planted = {c["scenario"] for c in cases}
        uncovered = sorted(AUDITED - planted - RULED - REVIEW_ONLY)
        self.assertEqual([], uncovered)

    def test_untargeted_calls_run_the_real_binary_unchanged(self):
        with tempfile.TemporaryDirectory() as d:
            real = Path(d) / "real"
            real.mkdir()
            tool = real / "sweep"
            tool.write_text('#!/bin/sh\necho "real $$ $*"\nexit 7\n')
            tool.chmod(0o755)
            env = dict(os.environ, MUSTFIRE_REAL_BIN=str(real), MUSTFIRE_MUTANT="undo_exits_1")
            p = subprocess.run([sys.executable, "-I", str(ROOT / "stress/mustfire/mutant.py"),
                                "sweep", "apply", d, "--yes"], env=env, capture_output=True, text=True)
        self.assertEqual(7, p.returncode)
        self.assertIn(f"apply {d} --yes", p.stdout)

    def test_an_unknown_planted_bug_refuses(self):
        env = dict(os.environ, MUSTFIRE_REAL_BIN="/nonexistent", MUSTFIRE_MUTANT="no_such_bug")
        p = subprocess.run([sys.executable, "-I", str(ROOT / "stress/mustfire/mutant.py"), "sweep"],
                           env=env, capture_output=True, text=True)
        self.assertEqual(70, p.returncode)


if __name__ == "__main__":
    unittest.main()
