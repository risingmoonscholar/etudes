"""The baseline rejects assertion and scenario loss without penalizing host limits."""
import contextlib
import importlib.util
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "baseline", Path(__file__).with_name("check-stress-baseline.py"))
baseline = importlib.util.module_from_spec(spec)
spec.loader.exec_module(baseline)


class BaselineTests(unittest.TestCase):
    def test_assertion_and_scenario_removal(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            scenarios = root / "stress/scenarios"
            scenarios.mkdir(parents=True)
            scenario = scenarios / "case.sh"
            scenario.write_text("pass first\npass second\n")
            catalog = [{"id": "case"}]
            floor = {"case": 2}
            def result():
                return {"cases": [{"id": "case", "passed": len(scenario.read_text().splitlines()),
                                   "unproven": 0}], "not_run": []}
            with patch.object(baseline, "ROOT", root):
                self.assertEqual([], baseline.check(floor, catalog, result()))
                scenario.write_text("pass first\n")
                self.assertTrue(baseline.check(floor, catalog, result()))
                scenario.write_text("pass first\npass second\n")
                self.assertEqual([], baseline.check(floor, catalog, result()))
                scenario.unlink()
                self.assertTrue(baseline.check(floor, [], {"cases": [], "not_run": []}))
                scenario.write_text("pass first\npass second\n")
                self.assertEqual([], baseline.check(floor, catalog, result()))

    def test_host_limits_and_unexecuted_tiers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            scenarios = root / "stress/scenarios"
            scenarios.mkdir(parents=True)
            (scenarios / "case.sh").touch()
            catalog = [{"id": "case"}]
            with patch.object(baseline, "ROOT", root), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual([], baseline.check({"case": 10}, catalog,
                    {"cases": [{"id": "case", "passed": 0, "unproven": 1}], "not_run": []}))
                self.assertEqual([], baseline.check({"case": 10}, catalog,
                    {"cases": [], "not_run": catalog}))
                self.assertTrue(baseline.check({"case": 9}, catalog,
                    {"cases": [], "not_run": catalog}, {"case": 10}))
                self.assertTrue(baseline.check({}, catalog,
                    {"cases": [], "not_run": catalog}, {"case": 10}))


if __name__ == "__main__":
    unittest.main()
