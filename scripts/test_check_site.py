"""Fault cases for the site build and the site check.

CheckSiteTest breaks a real build in a known way and expects check-site.py to
fail. ConverterTest feeds the Markdown converter each construct it claims to
render or to refuse, and holds it to that claim.
"""
import importlib.util
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


def load_build():
    spec = importlib.util.spec_from_file_location("build_site", BUILD)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


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

    def test_build_only_files_are_not_published(self):
        for name in ("tools.json", "docs.json", ".etudes-site-build"):
            self.assertFalse((self.clean / name).exists(), name)

    def test_rebuild_refuses_a_folder_it_did_not_make(self):
        foreign = Path(self.tmp.name) / "foreign"
        foreign.mkdir()
        (foreign / "keep.txt").write_text("not the site's\n")
        built = subprocess.run([sys.executable, str(BUILD), "--out", str(foreign)],
                               capture_output=True, text=True)
        self.assertEqual(built.returncode, 2, built.stdout + built.stderr)
        self.assertTrue((foreign / "keep.txt").is_file())

    def test_missing_anchor_fails(self):
        out = self.copy("anchor")
        self.edit(out / "docs/sweep/index.html", 'id="rules-the-tools-share"', 'id="renamed"')
        self.assertFails(self.check(out), "#rules-the-tools-share")

    def test_missing_page_fails(self):
        out = self.copy("page")
        shutil.rmtree(out / "docs/plan-binding")
        self.assertFails(self.check(out), "docs/plan-binding/")

    def test_link_to_missing_file_fails(self):
        out = self.copy("missing-file")
        self.edit(out / "docs/stash/index.html", 'href="../sweep/#rules-the-tools-share"',
                  'href="../sweep/gone.html"')
        self.assertFails(self.check(out), "docs/sweep/gone.html, which is not in the build")

    def test_link_outside_the_site_fails(self):
        out = self.copy("outside")
        self.edit(out / "docs/stash/index.html", 'href="../sweep/#rules-the-tools-share"',
                  'href="../../../elsewhere/"')
        self.assertFails(self.check(out), "points outside the site")

    def test_absolute_link_fails(self):
        out = self.copy("absolute")
        self.edit(out / "docs/stash/index.html", 'href="../sweep/#rules-the-tools-share"',
                  'href="/docs/sweep/#rules-the-tools-share"')
        self.assertFails(self.check(out), "absolute path")

    def test_each_raw_markdown_token_fails(self):
        for n, token in enumerate(("**", "```", "|---", "](")):
            with self.subTest(token=token):
                out = self.copy(f"leak-{n}")
                page = out / "docs/unpack/index.html"
                text = page.read_text(encoding="utf-8")
                page.write_text(text.replace("<p>", f"<p>left {token} here ", 1), encoding="utf-8")
                self.assertFails(self.check(out), f"raw Markdown {token!r}")

    def test_unlisted_page_fails(self):
        out = self.copy("unlisted")
        shutil.copytree(out / "docs/sweep", out / "docs/progress")
        self.assertFails(self.check(out), "docs/progress/ is published but is not on the public list")

    def test_drifted_transcripts_fail(self):
        out = self.copy("drift")
        with open(out / "transcripts.json", "a", encoding="utf-8") as f:
            f.write("\n")
        self.assertFails(self.check(out), "transcripts.json is not byte-identical")

    def test_drifted_home_page_fails(self):
        out = self.copy("home")
        with open(out / "index.html", "a", encoding="utf-8") as f:
            f.write("<!-- edited after the build -->\n")
        self.assertFails(self.check(out), "index.html is not byte-identical")

    def test_missing_font_license_fails(self):
        out = self.copy("ofl")
        (out / "assets/fonts/OFL.txt").unlink()
        self.assertFails(self.check(out), "without OFL.txt")

    def test_missing_font_file_fails(self):
        out = self.copy("font")
        (out / "assets/fonts/GeistMono-Variable.ttf").unlink()
        self.assertFails(self.check(out), "url(fonts/GeistMono-Variable.ttf) is not in the build")

    def test_duplicate_id_fails(self):
        out = self.copy("dupe")
        self.edit(out / "docs/sweep/index.html", 'id="what-it-does-not-protect"',
                  'id="tag-files-move-nothing"')
        self.assertFails(self.check(out), "duplicate id(s): tag-files-move-nothing")

    def test_empty_id_fails(self):
        out = self.copy("empty-id")
        self.edit(out / "docs/sweep/index.html", 'id="what-it-does-not-protect"', 'id=""')
        self.assertFails(self.check(out), "an element has an empty id")

    def test_published_data_file_fails(self):
        out = self.copy("data")
        shutil.copy(ROOT / "site/tools.json", out / "tools.json")
        self.assertFails(self.check(out), "tools.json is published")


class ConverterTest(unittest.TestCase):
    """Each construct either renders correctly or stops the build with its line."""

    @classmethod
    def setUpClass(cls):
        cls.mod = load_build()

    def converter(self):
        pages = [{"source": "docs/sweep.md", "dir": "docs/sweep"}]
        return self.mod.Converter("docs/x.md", "docs/x", self.mod.Linker(pages))

    def convert(self, text):
        return self.converter().convert(text)

    def refuses(self, text, line, words):
        with self.assertRaises(self.mod.BuildError) as caught:
            self.convert(text)
        self.assertIn(f"docs/x.md:{line}:", str(caught.exception))
        self.assertIn(words, str(caught.exception))

    # Rendered.

    def test_supported_markdown_renders(self):
        out = self.convert("# T\n\nA **b** and *c* with `d|e`.\n\n- one\n  two\n- three\n\n"
                           "| a | b |\n|---|---|\n| `x` | y |\n\n```sh\nls <dir>\n```\n")
        for piece in ("<strong>b</strong>", "<em>c</em>", "<code>d|e</code>", "<li>one\ntwo</li>",
                      "<td><code>x</code></td>", "ls &lt;dir&gt;"):
            self.assertIn(piece, out)

    def test_underscore_emphasis_renders(self):
        out = self.convert("Some _soft_ and __hard__ words, and snake_case_name stays.\n")
        self.assertIn("<em>soft</em>", out)
        self.assertIn("<strong>hard</strong>", out)
        self.assertIn("snake_case_name", out)

    def test_backslash_escape_renders_literally(self):
        out = self.convert("Not \\*emphasis\\*, a \\_name\\_ and a \\` tick.\n")
        self.assertIn("Not *emphasis*, a _name_ and a ` tick.", out)
        self.assertNotIn("<em>", out)
        self.assertNotIn("\\", out)

    def test_entities_are_kept_not_double_escaped(self):
        out = self.convert("Tom &amp; Jerry, &copy; 2026, &#233;, AT&T and &nosuch; stay.\n")
        self.assertIn("Tom &amp; Jerry, &copy; 2026, &#233;, AT&amp;T and &amp;nosuch; stay.", out)
        self.assertNotIn("&amp;amp;", out)

    def test_url_with_parentheses_renders(self):
        out = self.convert("See [Foo](https://en.wikipedia.org/wiki/Foo_(bar)) here.\n")
        self.assertIn('<a href="https://en.wikipedia.org/wiki/Foo_(bar)">Foo</a> here.', out)

    def test_ordered_list_keeps_its_start(self):
        out = self.convert("3. three\n4. four\n")
        self.assertIn('<ol start="3">', out)
        self.assertIn("<li>three</li>", out)
        self.assertIn("<ol>", self.convert("1. one\n2. two\n"))

    def test_autolink_renders(self):
        out = self.convert("Mail <mailto:a@example.com> or see <https://example.com/a?b=1&c=2>.\n")
        self.assertIn('<a href="mailto:a@example.com">mailto:a@example.com</a>', out)
        self.assertIn('<a href="https://example.com/a?b=1&amp;c=2">', out)

    def test_link_between_public_docs_becomes_a_site_link(self):
        out = self.convert("See [sweep](sweep.md#rules) and [notes](../CONTRIBUTING.md).\n")
        self.assertIn('href="../sweep/#rules"', out)
        self.assertIn('href="https://github.com/risingmoonscholar/etudes/blob/main/CONTRIBUTING.md"', out)

    def test_back_link_is_left_out_and_description_skips_it(self):
        conv = self.converter()
        out = conv.convert("# T\n\nBack to the [README](../README.md).\n\nThe real first paragraph.\n")
        self.assertNotIn("Back to the", out)
        self.assertEqual(conv.description, "The real first paragraph.")
        conv = self.converter()
        out = conv.convert("# T\n\nIt sorts files. Back to the [README](../README.md).\n")
        self.assertIn("<p>It sorts files.</p>", out)
        self.assertEqual(conv.description, "It sorts files.")

    def test_back_link_to_a_site_page_stays(self):
        out = self.convert("Back to the [sweep page](sweep.md).\n")
        self.assertIn('Back to the <a href="../sweep/">sweep page</a>.', out)

    def test_anchors_are_unique_and_never_empty(self):
        out = self.convert("# T\n\n## A\n\n## A\n\n## A-1\n\n## ???\n\n## ...\n")
        ids = re.findall(r'id="([^"]*)"', out)
        self.assertEqual(len(ids), len(set(ids)), ids)
        self.assertNotIn("", ids)
        self.assertEqual(ids[1:4], ["a", "a-1", "a-1-1"])

    # Refused, with the file and line.

    def test_fence_inside_a_list_item_stops_the_build(self):
        self.refuses("- item\n\n  ```sh\n  ls\n  ```\n", 3, "code fence")
        self.refuses("- item\n  ```sh\n  ls\n  ```\n", 2, "code fence")

    def test_indented_code_stops_the_build(self):
        self.refuses("Para.\n\n    code here\n", 3, "indented code")
        self.refuses("- item\n\n        code in item\n", 3, "indented code")

    def test_inline_html_stops_the_build(self):
        self.refuses("A <b>bold</b> word.\n", 1, "inline HTML")
        self.refuses("Text\nwith <!-- a comment -->.\n", 1, "inline HTML")

    def test_link_with_a_title_stops_the_build(self):
        self.refuses('A [link](https://example.com "title").\n', 1, "title")

    def test_javascript_link_stops_the_build(self):
        with self.assertRaises(self.mod.BuildError) as caught:
            self.convert("A [link](javascript:alert(1)).\n")
        self.assertIn("scheme javascript", str(caught.exception))

    def test_other_unsupported_markdown_stops_the_build(self):
        for text, words in (("> quoted\n", "block quotes"), ("<div>raw</div>\n", "HTML"),
                            ("![alt](a.png)\n", "images"), ("A ![alt](a.png) inline.\n", "images"),
                            ("a\n\n---\n", "rules"), ("- one\n  - nested\n", "nested"),
                            ("[ref]: https://example.com\n", "reference"),
                            ("See [x][ref].\n", "reference")):
            with self.subTest(text=text), self.assertRaises(self.mod.BuildError) as caught:
                self.convert(text)
            self.assertIn(words, str(caught.exception))
            self.assertRegex(str(caught.exception), r"docs/x\.md:\d+:")

    def test_link_to_missing_repo_file_stops_the_build(self):
        with self.assertRaises(self.mod.BuildError):
            self.convert("See [gone](no-such-file.md).\n")


if __name__ == "__main__":
    unittest.main()
