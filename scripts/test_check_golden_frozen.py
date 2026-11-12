"""Fault cases for the golden-file check, each in a throwaway git repository."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("check-golden-frozen.py")
GOLDEN = "crates/etude-core/tests/golden"
TEST = "crates/etude-core/tests/plan_golden.rs"


class GoldenFrozen(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.repo = Path(tmp.name)
        (self.repo / "scripts").mkdir()
        shutil.copy(SCRIPT, self.repo / "scripts")
        self.write("README", "fixture\n")
        self.git("init", "-q", "-b", "main")
        self.commit("first")
        self.empty = self.head()
        self.write(TEST, "// golden test\n")
        self.write(f"{GOLDEN}/a__build.json", "{}\n")
        self.write(f"{GOLDEN}/b__build.json", "{}\n")
        self.commit("add the golden test")
        self.base = self.head()

    def write(self, name, text):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def git(self, *args):
        return subprocess.run(
            ["git", "-c", "user.name=fixture", "-c", "user.email=fixture@example.invalid",
             "-c", "core.hooksPath=/dev/null", *args],
            cwd=self.repo, check=True, capture_output=True, text=True).stdout

    def head(self):
        return self.git("rev-parse", "HEAD").strip()

    def commit(self, message):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", message)

    def check(self, base=None):
        result = subprocess.run(
            [sys.executable, "-I", "scripts/check-golden-frozen.py", "--base", base or self.base],
            cwd=self.repo, capture_output=True, text=True)
        return result.returncode, result.stdout + result.stderr

    def test_an_unreadable_base_fails(self):
        code, text = self.check("no-such-revision")
        self.assertEqual(code, 1, text)
        self.assertIn("FAIL", text)

    def test_an_unchanged_tree_passes(self):
        code, text = self.check()
        self.assertEqual(code, 0, text)
        self.assertIn("no golden file changed", text)

    def test_a_change_outside_the_golden_files_passes(self):
        self.write("README", "changed\n")
        self.commit("change")
        code, text = self.check()
        self.assertEqual(code, 0, text)

    def test_a_changed_file_with_no_trailer_fails(self):
        self.write(f"{GOLDEN}/a__build.json", "{\"changed\": true}\n")
        self.commit("change")
        code, text = self.check()
        self.assertEqual(code, 1, text)
        self.assertIn(f"FAIL {GOLDEN}/a__build.json", text)

    def test_a_changed_file_with_a_trailer_passes(self):
        self.write(f"{GOLDEN}/a__build.json", "{\"changed\": true}\n")
        self.commit(f"change\n\nGolden-Change: {GOLDEN}/a__build.json: the screenshot rule reads more languages")
        code, text = self.check()
        self.assertEqual(code, 0, text)
        self.assertIn("reads more languages", text)

    def test_a_trailer_for_another_file_does_not_count(self):
        self.write(f"{GOLDEN}/a__build.json", "{\"changed\": true}\n")
        self.commit(f"change\n\nGolden-Change: {GOLDEN}/b__build.json: unrelated")
        code, text = self.check()
        self.assertEqual(code, 1, text)
        self.assertIn(f"FAIL {GOLDEN}/a__build.json", text)

    def test_a_trailer_with_no_reason_fails(self):
        self.write(f"{GOLDEN}/a__build.json", "{\"changed\": true}\n")
        self.commit(f"change\n\nGolden-Change: {GOLDEN}/a__build.json:")
        code, text = self.check()
        self.assertEqual(code, 1, text)
        self.assertIn("no reason", text)

    def test_a_deleted_file_with_no_trailer_fails(self):
        (self.repo / GOLDEN / "b__build.json").unlink()
        self.commit("delete")
        code, text = self.check()
        self.assertEqual(code, 1, text)
        self.assertIn("deleted", text)

    def test_the_commit_that_adds_the_test_is_the_baseline(self):
        code, text = self.check(self.empty)
        self.assertEqual(code, 0, text)
        self.assertIn("(baseline)", text)

    def test_a_golden_file_added_later_needs_a_trailer(self):
        self.write(f"{GOLDEN}/c__build.json", "{}\n")
        self.commit("add another")
        code, text = self.check(self.empty)
        self.assertEqual(code, 1, text)
        self.assertIn(f"FAIL {GOLDEN}/c__build.json: added", text)


if __name__ == "__main__":
    unittest.main()
