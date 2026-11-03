"""Fault cases for the agent demo check, each in a throwaway git repository."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("check-agent-demo.py")


class AgentDemo(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.repo = Path(tmp.name)
        (self.repo / "scripts").mkdir()
        shutil.copy(SCRIPT, self.repo / "scripts")
        for crate in ("sweep-cli", "unpack-cli", "etude-core"):
            self.write(f"crates/{crate}/src/main.rs", "fn main() {}\n")
        self.write("crates/unpack-cli/tests/t.rs", "\n")
        self.write("stress/scenarios/old.sh", "$SWEEP $UNPACK\n")
        self.git("init", "-q", "-b", "main")
        self.commit("first")
        self.base = self.git("rev-parse", "HEAD").strip()

    def write(self, name, text):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def git(self, *args):
        return subprocess.run(
            ["git", "-c", "user.name=fixture", "-c", "user.email=fixture@example.invalid",
             "-c", "core.hooksPath=/dev/null", *args],
            cwd=self.repo, check=True, capture_output=True, text=True).stdout

    def commit(self, message):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", message)

    def check(self, base=None):
        result = subprocess.run(
            [sys.executable, "-I", "scripts/check-agent-demo.py", "--base", base or self.base],
            cwd=self.repo, capture_output=True, text=True)
        return result.returncode, result.stdout + result.stderr

    def test_an_unreadable_base_fails(self):
        code, text = self.check("no-such-revision")
        self.assertEqual(code, 1, text)
        self.assertIn("FAIL", text)

    def test_comparing_a_commit_with_itself_fails(self):
        code, text = self.check()
        self.assertEqual(code, 1, text)
        self.assertIn("nothing was compared", text)

    def test_a_tool_change_with_no_scenario_fails(self):
        self.write("crates/unpack-cli/src/main.rs", "fn main() { }\n")
        self.commit("change")
        code, text = self.check()
        self.assertEqual(code, 1, text)
        self.assertIn("$UNPACK", text)

    def test_a_tool_change_with_a_scenario_passes(self):
        self.write("crates/unpack-cli/src/main.rs", "fn main() { }\n")
        self.write("stress/scenarios/new.sh", "$UNPACK\n")
        self.commit("change")
        code, text = self.check()
        self.assertEqual(code, 0, text)

    def test_a_scenario_for_another_tool_does_not_count(self):
        self.write("crates/unpack-cli/src/main.rs", "fn main() { }\n")
        self.write("stress/scenarios/new.sh", "$SWEEP\n")
        self.commit("change")
        self.assertEqual(self.check()[0], 1)

    def test_a_test_only_change_is_exempt(self):
        self.write("crates/unpack-cli/tests/t.rs", "// more\n")
        self.commit("change")
        code, text = self.check()
        self.assertEqual(code, 0, text)
        self.assertIn("no tool code changed", text)

    def test_a_shared_crate_needs_every_tool_driven(self):
        self.write("crates/etude-core/src/main.rs", "fn main() { }\n")
        self.write("stress/scenarios/new.sh", "$SWEEP\n")
        self.commit("change")
        code, text = self.check()
        self.assertEqual(code, 1, text)
        self.assertIn("FAIL $UNPACK", text)

    def test_a_new_tool_is_covered_from_its_first_change(self):
        self.write("crates/pack-cli/src/main.rs", "fn main() {}\n")
        self.commit("change")
        code, text = self.check()
        self.assertEqual(code, 1, text)
        self.assertIn("FAIL $PACK", text)


if __name__ == "__main__":
    unittest.main()
