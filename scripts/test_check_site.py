"""Fault cases for the site build and the site check.

CheckSiteTest breaks a real build in a known way and expects check-site.py to
fail. ConverterTest feeds the Markdown converter each construct it claims to
render or to refuse, and holds it to that claim.
"""
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
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

    def test_drifted_demo_page_fails(self):
        out = self.copy("demo")
        with open(out / "demo/index.html", "a", encoding="utf-8") as f:
            f.write("<!-- edited after the build -->\n")
        self.assertFails(self.check(out), "demo/index.html is not byte-identical")

    def test_edited_home_page_fails(self):
        out = self.copy("home")
        with open(out / "index.html", "a", encoding="utf-8") as f:
            f.write("<!-- edited after the build -->\n")
        self.assertFails(self.check(out), "index.html is not what scripts/build-site.py renders")

    def test_install_version_other_than_cargo_toml_fails(self):
        out = self.copy("version")
        version = tomllib.loads((ROOT / "crates/stash-cli/Cargo.toml").read_text())["package"]["version"]
        self.edit(out / "index.html", f"--tag stash-v{version} stash-cli", "--tag stash-v0.0.1 stash-cli")
        self.assertFails(self.check(out),
                         f"the install block names stash 0.0.1; crates/stash-cli/Cargo.toml declares {version}")

    def test_install_block_missing_a_tool_fails(self):
        out = self.copy("missing-tool")
        page = out / "index.html"
        text, n = re.subn(r"cargo install --locked --git \S+ --tag unpack-v\S+ unpack-cli", "", page.read_text())
        self.assertGreater(n, 0)
        page.write_text(text)
        self.assertFails(self.check(out), "the install block has no install line for unpack")

    def test_install_from_another_repo_fails(self):
        out = self.copy("repo")
        self.edit(out / "index.html", "--git https://github.com/risingmoonscholar/etudes --tag sweep",
                  "--git https://example.com/etudes --tag sweep")
        self.assertFails(self.check(out), "installs from https://example.com/etudes")

    def test_home_output_other_than_transcripts_fails(self):
        out = self.copy("home-output")
        self.edit(out / "index.html", "Stashed 112 items.", "Stashed 113 items.")
        self.assertFails(self.check(out), "the home page's 'stash-put' output differs from site/transcripts.json")

    def test_changelog_data_is_not_published(self):
        # The changelog folder holds built pages only: no entry data, fixture or log.
        self.assertTrue((self.clean / "changelog/index.html").is_file())
        published = [p.relative_to(self.clean).as_posix() for p in (self.clean / "changelog").rglob("*")
                     if p.is_file()]
        self.assertTrue(published)
        for rel in published:
            self.assertRegex(rel, r"^changelog/([a-z]+-\d+\.\d+\.\d+/)?index\.html$")

    def test_published_changelog_data_fails(self):
        out = self.copy("changelog-data")
        shutil.copy(ROOT / "site/changelog/sweep-0.5.5.json", out / "changelog/sweep-0.5.5.json")
        self.assertFails(self.check(out), "changelog/sweep-0.5.5.json is published")

    def test_edited_changelog_page_fails(self):
        out = self.copy("changelog-page")
        self.edit(out / "changelog/sweep-0.5.5/index.html", "Moved 2 files.", "Moved 3 files.")
        self.assertFails(self.check(out), "the published changelog/sweep-0.5.5/index.html is not what")

    def test_whats_new_showing_an_older_entry_fails(self):
        out = self.copy("whats-new")
        build = load_build()
        entries = build.changelog_entries()
        self.assertGreater(len(entries), 1, "needs two entries to swap")
        page = out / "index.html"
        page.write_text(page.read_text().replace(build.render_entry(entries[0]),
                                                 build.render_entry(entries[-1])))
        self.assertFails(self.check(out), "What's new does not show the newest entry")

    def test_topbar_changelog_link_is_the_changelog_page(self):
        for page in ("index.html", "docs/index.html", "changelog/index.html"):
            text = (self.clean / page).read_text()
            self.assertRegex(text, r'<a href="(\.\./)*changelog/"[^>]*>Changelog</a>')
            self.assertNotIn('blob/main/CHANGELOG.md">Changelog</a>', text)

    def test_install_works_without_javascript(self):
        # Every install line sits in the page as text; the tab row and the
        # copy buttons start hidden and only the script shows them.
        page = (self.clean / "index.html").read_text()
        self.assertIn('role="tablist" aria-label="Which tools to install" hidden', page)
        self.assertEqual(page.count('class="copy"'), page.count('class="copy" data-copy='))
        for button in re.findall(r'<button type="button" class="copy"[^>]*>', page):
            self.assertIn(" hidden", button)
        for tool in ("sweep", "stash", "unpack"):
            self.assertIn(f'id="install-{tool}"', page)
            self.assertNotIn(f'id="install-{tool}" hidden', page)

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


def load_check():
    spec = importlib.util.spec_from_file_location("check_site", CHECK)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


class EntryTest(unittest.TestCase):
    """The pages job has the tags the release state is read from."""

    def test_pages_job_fetches_tags_before_it_checks_the_site(self):
        # build-site.py reads the release's tag state from local tags. The job
        # that publishes must have them, or a "not tagged yet" footer would go
        # out on the release.
        text = (ROOT / ".github/workflows/pages.yml").read_text()
        checkout = text.index("uses: actions/checkout@v4")
        check = text.index("python3 scripts/check-site.py --dir _site")
        self.assertLess(checkout, check)
        step = text[checkout:text.index("- name:", checkout)]
        self.assertRegex(step, r"\n\s+fetch-depth: 0\n")

    def test_published_entry_matches_its_data(self):
        build = load_build()
        entry = build.latest_entry()
        page = build.render_entry(entry)
        for side in ("before", "after"):
            for run in entry[side]["runs"]:
                self.assertIn(run["command"], page)
                for line in run["output"]:
                    self.assertIn(build.html.escape(line, quote=False), page)
                self.assertIn(f"exit {run['exit']}", page)


def load_runs():
    spec = importlib.util.spec_from_file_location("release_runs", ROOT / "scripts/release-runs.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


class ChangelogDataTest(unittest.TestCase):
    """A changelog entry is what scripts/release-runs.py derives from its raw logs."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="etudes-changelog-test-")
        self.folder = Path(self.tmp.name) / "changelog"
        shutil.copytree(ROOT / "site/changelog", self.folder)
        self.entry_path = self.folder / "sweep-0.5.5.json"

    def tearDown(self):
        self.tmp.cleanup()

    def problems(self):
        return load_check().check_entries(self.folder)[0]

    def change(self, path, fn):
        data = json.loads(path.read_text())
        fn(data)
        path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n")

    def assertProblem(self, needle):
        problems = self.problems()
        self.assertTrue(any(needle in p for p in problems), problems)

    def test_committed_entries_pass(self):
        self.assertEqual(self.problems(), [])

    def test_hand_edited_output_line_fails(self):
        self.change(self.entry_path, lambda e: e["after"]["runs"][1]["output"].__setitem__(0, "Moved 3 files."))
        self.assertProblem("sweep-0.5.5.json differs from its raw logs in after")

    def test_hand_edited_exit_code_fails(self):
        self.change(self.entry_path, lambda e: e["before"]["runs"][0].__setitem__("exit", 0))
        self.assertProblem("differs from its raw logs in before")

    def test_hand_edited_next_output_fails(self):
        self.change(self.entry_path, lambda e: e["next"]["output"].append("All good."))
        self.assertProblem("differs from its raw logs in next")

    def test_hand_edited_result_file_fails(self):
        self.change(self.entry_path, lambda e: e["after"]["result"]["files"].append("extra.pdf"))
        self.assertProblem("differs from its raw logs in after")

    def test_hand_edited_runs_note_fails(self):
        self.change(self.entry_path,
                    lambda e: e.__setitem__("runs_note", e["runs_note"].replace("2 new PDFs", "3 new PDFs")))
        self.assertProblem("differs from its raw logs in runs_note")

    def test_log_with_a_machine_path_fails(self):
        log = self.folder / "logs/sweep-0.5.5/after-2.txt"
        log.write_text(log.read_text().replace("<scratch>", "/private/var/folders/xy/T/run"))
        self.assertProblem("holds a machine's own folder path")

    def test_hand_written_fields_may_change(self):
        def edit(e):
            e["headline"] = "A different headline"
            e["lead"] = [{"text": "Another lead."}]
            e["next"]["note"] = [{"text": "Another note."}]
        self.change(self.entry_path, edit)
        self.assertEqual(self.problems(), [])

    def test_edited_raw_log_fails(self):
        log = self.folder / "logs/sweep-0.5.5/after-2.txt"
        log.write_text(log.read_text().replace("Moved 2 files.", "Moved 3 files."))
        self.assertProblem("after-2.txt differ from the sha256")

    def test_entry_without_fixture_fails(self):
        (self.folder / "sweep-0.5.5.fixture").unlink()
        self.assertProblem("sweep-0.5.5.json has no fixture")

    def test_entry_without_logs_fails(self):
        shutil.rmtree(self.folder / "logs/sweep-0.5.5")
        self.assertProblem("sweep-0.5.5.json has no raw logs")

    def test_fixture_without_entry_fails(self):
        shutil.copy(self.folder / "sweep-0.5.5.fixture", self.folder / "sweep-9.9.9.fixture")
        self.assertProblem("sweep-9.9.9.fixture has no entry")

    def test_fixture_commands_changed_after_the_runs_fails(self):
        self.change(self.folder / "sweep-0.5.5.fixture",
                    lambda f: f["runs"][0].__setitem__("command", "sweep inbox --json"))
        self.assertProblem("changed its setup, commands or result globs after the runs")

    def test_fixture_result_glob_changed_after_the_runs_fails(self):
        self.change(self.folder / "sweep-0.5.5.fixture",
                    lambda f: f["result"]["after"].__setitem__("glob", "inbox/*"))
        self.assertProblem("changed its setup, commands or result globs after the runs")

    def test_fixture_cut_that_matches_nothing_fails(self):
        self.change(self.folder / "sweep-0.5.5.fixture",
                    lambda f: f["runs"][1]["show"].__setitem__("after", ["^Moved 9 files"]))
        self.assertProblem("matches no line")

    def test_untagged_after_build_not_on_head_fails(self):
        # sweep-v0.5.4's commit sits on a release branch, not on main. With no
        # sweep-v9.9.9 tag, an entry that ran there is a candidate off HEAD.
        rev = subprocess.run(["git", "rev-parse", "sweep-v0.5.4^{commit}"], cwd=ROOT,
                             capture_output=True, text=True).stdout.strip()
        anc = subprocess.run(["git", "merge-base", "--is-ancestor", rev, "HEAD"], cwd=ROOT)
        if not rev or anc.returncode != 1:
            self.skipTest("needs the sweep-v0.5.4 tag off HEAD's history")
        problems = load_check().check_builds(
            {"tool": "sweep", "version": "9.9.9",
             "builds": {"before": {}, "after": {"commit": rev[:7], "rev": rev}}})
        self.assertTrue(any("is not an ancestor of HEAD" in p for p in problems), problems)

    def test_before_tag_on_the_wrong_commit_fails(self):
        entry = json.loads((ROOT / "site/changelog/sweep-0.5.5.json").read_text())
        entry["builds"]["before"]["rev"] = "0" * 40
        problems = load_check().check_builds(entry)
        self.assertTrue(any("the before build is sweep-v0.5.4, which names" in p for p in problems), problems)

    def test_entries_store_no_tag_state_or_date(self):
        for path in (ROOT / "site/changelog").glob("*.json"):
            entry = json.loads(path.read_text())
            self.assertNotIn("date", entry, path.name)
            self.assertNotIn("footer", entry, path.name)
            self.assertNotIn("tagged", entry["builds"]["after"], path.name)


class ReleaseStateTest(unittest.TestCase):
    """Tag state and date come from git at build time. Tried in a scratch clone
    with throwaway tags; nothing is tagged or committed in this repository."""

    TAG = "sweep-v0.5.5"

    @classmethod
    def setUpClass(cls):
        if subprocess.run(["git", "rev-parse", "--verify", "--quiet", f"refs/tags/{cls.TAG}"],
                          cwd=ROOT, capture_output=True).returncode == 0:
            raise unittest.SkipTest(f"{cls.TAG} exists here already")
        cls.tmp = tempfile.TemporaryDirectory(prefix="etudes-release-state-")
        cls.clone = Path(cls.tmp.name) / "clone"
        head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True,
                              text=True, check=True).stdout.strip()
        subprocess.run(["git", "clone", "--quiet", "--shared", "--no-checkout", str(ROOT), str(cls.clone)],
                       check=True)
        cls.git("checkout", "--quiet", "--detach", head)
        # The scripts and data as they are in this checkout, on top of HEAD.
        for folder in ("scripts", "site"):
            shutil.rmtree(cls.clone / folder)
            shutil.copytree(ROOT / folder, cls.clone / folder)
        cls.git("add", "-A")
        cls.git("commit", "--quiet", "--allow-empty", "-m", "the checkout as it is")
        cls.entry = json.loads((cls.clone / "site/changelog/sweep-0.5.5.json").read_text())
        cls.rev = cls.entry["builds"]["after"]["rev"]

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    @classmethod
    def git(cls, *args):
        return subprocess.run(["git", "-c", "user.name=test", "-c", "user.email=test@example.invalid",
                               "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false", *args],
                              cwd=cls.clone, capture_output=True, text=True, check=True).stdout.strip()

    def tearDown(self):
        subprocess.run(["git", "tag", "-d", self.TAG], cwd=self.clone, capture_output=True)

    def build(self):
        spec = importlib.util.spec_from_file_location("build_site_clone", self.clone / "scripts/build-site.py")
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        return mod

    def commit_on(self, base, path, text):
        """A commit on BASE that changes PATH; HEAD goes back where it was."""
        head = self.git("rev-parse", "HEAD")
        self.git("checkout", "--quiet", "--detach", base)
        with open(self.clone / path, "a") as f:
            f.write(text)
        self.git("commit", "--quiet", "-am", "change")
        rev = self.git("rev-parse", "HEAD")
        self.git("checkout", "--quiet", "--detach", head)
        return rev

    def test_untagged_is_a_release_candidate_without_a_date(self):
        build = self.build()
        state = build.release_state(self.entry)
        self.assertFalse(state["tagged"])
        self.assertIn("not tagged yet", build.footer(self.entry))
        self.assertIn("Release candidate", build.release_meta(self.entry))
        self.assertNotIn("<time", build.release_meta(self.entry))

    def test_tag_on_the_run_commit_is_tagged_with_its_date(self):
        self.git("tag", "-a", "-m", "release", self.TAG, self.rev)
        build = self.build()
        state = build.release_state(self.entry)
        self.assertTrue(state["tagged"])
        date = self.git("for-each-ref", "--format=%(creatordate:short)", f"refs/tags/{self.TAG}")
        self.assertIn(f'<time datetime="{date}">', build.release_meta(self.entry))
        self.assertIn(f"tag {self.TAG} (commit {self.rev[:7]})", build.footer(self.entry))
        self.assertNotIn("not tagged", build.footer(self.entry))

    def test_tag_on_a_commit_with_the_same_sweep_source_is_tagged(self):
        tagged = self.commit_on(self.rev, "README.md", "\nA docs change only.\n")
        self.git("tag", self.TAG, tagged)
        build = self.build()
        self.assertTrue(build.release_state(self.entry)["tagged"])
        self.assertIn(f"built and run at commit {self.rev[:7]}, which has the same sweep source",
                      build.footer(self.entry))

    def test_tag_on_a_commit_with_other_sweep_source_stops_the_build(self):
        for path in ("crates/sweep-cli/src/main.rs", "crates/etude-core/src/lib.rs", "Cargo.lock"):
            with self.subTest(path=path):
                tagged = self.commit_on(self.rev, path, "\n")
                self.git("tag", "-f", self.TAG, tagged)
                build = self.build()
                with self.assertRaises(build.BuildError) as caught:
                    build.release_state(self.entry)
                self.assertIn("whose sweep source differs", str(caught.exception))

    def test_tagged_site_passes_the_site_check(self):
        # At tag time pages.yml builds and checks the tagged tree. The entry
        # made before the tag must not stop it.
        self.git("tag", "-a", "-m", "release", self.TAG, self.rev)
        done = subprocess.run([sys.executable, str(self.clone / "scripts/check-site.py")],
                              cwd=self.clone, capture_output=True, text=True)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        built = Path(self.tmp.name) / "site"
        subprocess.run([sys.executable, str(self.clone / "scripts/build-site.py"), "--out", str(built)],
                       check=True, capture_output=True)
        page = (built / "changelog/sweep-0.5.5/index.html").read_text()
        self.assertIn(f"tag {self.TAG} (commit {self.rev[:7]})", page)
        self.assertNotIn("Release candidate", page)


class KeychainGuardTest(unittest.TestCase):
    """No run of release-runs.py can reach the login keychain."""

    @classmethod
    def setUpClass(cls):
        cls.runs = load_runs()

    def rev(self, ref):
        done = subprocess.run(["git", "rev-parse", "--verify", "--quiet", f"{ref}^{{commit}}"], cwd=ROOT,
                              capture_output=True, text=True)
        if done.returncode != 0:
            self.skipTest(f"this clone has no {ref}")
        return done.stdout.strip()

    def test_a_ref_without_key_support_is_refused(self):
        for tool, ref in (("sweep", "v0.4.0"), ("stash", "v0.5.1"), ("sweep", "sweep-v0.5.2"),
                          ("stash", "stash-v0.5.2")):
            with self.subTest(tool=tool, ref=ref), self.assertRaises(self.runs.RunError) as caught:
                self.runs.key_guard(tool, ref, self.rev(ref))
            self.assertIn("does not read ETUDE_JOURNAL_KEY", str(caught.exception))

    def test_refs_with_key_support_and_unpack_pass(self):
        self.runs.key_guard("sweep", "sweep-v0.5.4", self.rev("sweep-v0.5.4"))
        self.runs.key_guard("stash", "stash-v0.5.3", self.rev("stash-v0.5.3"))
        self.runs.key_guard("unpack", "v0.5.1", self.rev("v0.5.1"))

    def test_the_script_refuses_before_it_builds(self):
        self.rev("v0.4.0")
        done = subprocess.run([sys.executable, str(ROOT / "scripts/release-runs.py"), "sweep", "0.5.5",
                               "--before", "v0.4.0", "--after", "HEAD"], capture_output=True, text=True)
        self.assertEqual(done.returncode, 1, done.stdout + done.stderr)
        self.assertIn("does not read ETUDE_JOURNAL_KEY", done.stderr)
        self.assertIn("Nothing was built or run", done.stderr)
        self.assertNotIn("building", done.stderr)

    @unittest.skipUnless(Path("/usr/bin/sandbox-exec").exists(), "macOS sandbox-exec only")
    def test_runs_cannot_exec_security_or_write_outside(self):
        with tempfile.TemporaryDirectory(prefix="etudes-sandbox-test-") as tmp:
            base = Path(tmp) / "scratch"
            base.mkdir()
            scratch = self.runs.Scratch(base, "t", "0" * 64)
            # By absolute path: the sandbox refuses the exec.
            done = scratch.exec([self.runs.SECURITY, "help"])
            self.assertNotEqual(done.returncode, 0)
            self.assertIn(b"Operation not permitted", done.stdout)
            # By name: the shim on the front of PATH refuses first.
            done = scratch.exec(["/bin/sh", "-c", "security help"])
            self.assertEqual(done.returncode, 77)
            self.assertIn(b"security: refused", done.stdout)
            # A write outside the scratch folder is refused; inside works.
            outside = Path(tmp) / "outside.txt"
            done = scratch.exec(["/usr/bin/touch", str(outside)])
            self.assertNotEqual(done.returncode, 0)
            self.assertFalse(outside.exists())
            done = scratch.exec(["/usr/bin/touch", str(scratch.work / "inside.txt")])
            self.assertEqual(done.returncode, 0, done.stdout)

    def test_logs_write_the_scratch_folder_as_a_placeholder(self):
        with tempfile.TemporaryDirectory(prefix="etudes-normalize-test-") as tmp:
            scratch = self.runs.Scratch(Path(tmp), "t", "0" * 64)
            raw = f"Undo with: sweep undo '{os.path.realpath(scratch.root)}/work/inbox'\n".encode()
            self.assertEqual(scratch.normalize(raw), b"Undo with: sweep undo '<scratch>/work/inbox'\n")

    def test_scratch_with_an_unreadable_folder_is_removed(self):
        tmp = tempfile.mkdtemp(prefix="etudes-rmtree-test-")
        locked = Path(tmp) / "a" / "locked"
        locked.mkdir(parents=True)
        (locked / "f.txt").write_text("x")
        locked.chmod(0)
        self.runs.force_rmtree(tmp)
        self.assertFalse(Path(tmp).exists())


class ReleaseRunsTest(unittest.TestCase):
    """The cuts, JSON keys and runs note scripts/release-runs.py writes."""

    @classmethod
    def setUpClass(cls):
        cls.runs = load_runs()

    def test_cut_keeps_whole_lines_in_order(self):
        lines = ["", "a one", "b two", "a three"]
        self.assertEqual(self.runs.cut(lines, ["^a", "^a"], "x"), ["a one", "a three"])
        self.assertEqual(self.runs.cut(lines, ["two"], "x"), ["b two"])

    def test_cut_that_matches_nothing_refuses(self):
        with self.assertRaises(self.runs.RunError) as caught:
            self.runs.cut(["a", "b"], ["^b", "^a"], "after run 1")
        self.assertIn("after run 1", str(caught.exception))

    def test_cuts_do_not_change_the_runs_digest(self):
        fixture = json.loads((ROOT / "site/changelog/sweep-0.5.5.fixture").read_text())
        digest = self.runs.runs_digest(fixture)
        fixture["runs"][0]["show"]["after"] = ["."]
        self.assertEqual(self.runs.runs_digest(fixture), digest)
        fixture["setup"].append("touch extra")
        self.assertNotEqual(self.runs.runs_digest(fixture), digest)

    def test_json_fields_read_nested_keys(self):
        text = '{"details": {"clear": false}, "moved": 4}\n'
        fields = self.runs.json_fields(text, ["clear", "details.clear"], "x")
        self.assertEqual(fields, [{"path": "clear", "present": False, "value": None},
                                  {"path": "details.clear", "present": True, "value": False}])

    def test_runs_note_says_shortened_only_when_a_line_was_cut(self):
        record = json.loads((ROOT / "site/changelog/logs/sweep-0.5.5/record.json").read_text())
        fixture = json.loads((ROOT / "site/changelog/sweep-0.5.5.fixture").read_text())
        logs = ROOT / "site/changelog/logs/sweep-0.5.5"

        def every_line(name):
            return ["^" + re.escape(line) + "$" for line in self.runs.log_lines((logs / name).read_text())]
        for n, run in enumerate(fixture["runs"]):
            for side in ("before", "after"):
                run["show"][side] = every_line(record["sides"][side]["runs"][n]["log"])
        fixture["next"]["show"] = every_line("next.txt")
        entry = self.runs.entry_from_record(record, fixture, {}, lambda n: (logs / n).read_text())
        self.assertNotIn("shortened", entry["runs_note"])


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
