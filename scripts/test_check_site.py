"""Fault cases for the site check: each breaks a real build in a known way."""
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BUILD = ROOT / "scripts" / "build-site.py"
CHECK = ROOT / "scripts" / "check-site.py"


class CheckSiteTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="etudes-site-test-")
        cls.clean = Path(cls.tmp.name) / "clean"
        built = subprocess.run([sys.executable, str(BUILD), "--out", str(cls.clean)],
                               capture_output=True, text=True)
        if built.returncode != 0:
            raise AssertionError("the site does not build:\n" + built.stdout + built.stderr)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def copy(self, name):
        target = Path(self.tmp.name) / name
        shutil.copytree(self.clean, target)
        return target

    def check(self, out):
        return subprocess.run([sys.executable, str(CHECK), "--dir", str(out)],
                              capture_output=True, text=True)

    def edit(self, path, old, new):
        text = path.read_text(encoding="utf-8")
        self.assertIn(old, text, f"fixture text not found in {path}")
        path.write_text(text.replace(old, new, 1), encoding="utf-8")

    def assertFails(self, result, needle):
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(needle, result.stdout)

    def test_clean_build_passes(self):
        result = self.check(self.clean)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("internal links and anchors", result.stdout)

    def test_missing_anchor_fails(self):
        out = self.copy("anchor")
        self.edit(out / "docs/sweep/index.html", 'id="rules-the-tools-share"', 'id="renamed"')
        self.assertFails(self.check(out), "#rules-the-tools-share")

    def test_missing_page_fails(self):
        out = self.copy("page")
        shutil.rmtree(out / "docs/plan-binding")
        self.assertFails(self.check(out), "docs/plan-binding/")

    def test_raw_markdown_fails(self):
        out = self.copy("leak")
        page = out / "docs/unpack/index.html"
        text = page.read_text(encoding="utf-8")
        page.write_text(re.sub(r"<strong>(.*?)</strong>", r"**\1**", text, count=1), encoding="utf-8")
        self.assertFails(self.check(out), "raw Markdown '**'")

    def test_unlisted_page_fails(self):
        out = self.copy("unlisted")
        shutil.copytree(out / "docs/sweep", out / "docs/progress")
        self.assertFails(self.check(out), "docs/progress/ is published but is not on the public list")

    def test_drifted_transcripts_fail(self):
        out = self.copy("drift")
        with open(out / "transcripts.json", "a", encoding="utf-8") as f:
            f.write("\n")
        self.assertFails(self.check(out), "transcripts.json is not byte-identical")

    def test_absolute_link_fails(self):
        out = self.copy("absolute")
        self.edit(out / "docs/stash/index.html", 'href="../sweep/#rules-the-tools-share"',
                  'href="/docs/sweep/#rules-the-tools-share"')
        self.assertFails(self.check(out), "absolute path")


class ConverterRefusesTest(unittest.TestCase):
    """Markdown the converter does not cover stops the build instead of leaking."""

    @classmethod
    def setUpClass(cls):
        import importlib.util
        spec = importlib.util.spec_from_file_location("build_site", BUILD)
        cls.mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.mod)

    def convert(self, text):
        linker = self.mod.Linker([])
        return self.mod.Converter("docs/x.md", "docs/x", linker).convert(text)

    def test_supported_markdown_renders(self):
        out = self.convert("# T\n\nA **b** and *c* with `d|e`.\n\n- one\n  two\n- three\n\n"
                           "| a | b |\n|---|---|\n| `x` | y |\n\n```sh\nls <dir>\n```\n")
        for piece in ("<strong>b</strong>", "<em>c</em>", "<code>d|e</code>", "<li>one\ntwo</li>",
                      "<td><code>x</code></td>", "ls &lt;dir&gt;"):
            self.assertIn(piece, out)

    def test_unsupported_markdown_stops_the_build(self):
        for text in ("> quoted\n", "<div>raw</div>\n", "![alt](a.png)\n", "a\n\n---\n",
                     "- one\n  - nested\n", "[ref]: https://example.com\n"):
            with self.subTest(text=text), self.assertRaises(self.mod.BuildError):
                self.convert(text)

    def test_link_to_missing_repo_file_stops_the_build(self):
        with self.assertRaises(self.mod.BuildError):
            self.convert("See [gone](no-such-file.md).\n")


if __name__ == "__main__":
    unittest.main()
