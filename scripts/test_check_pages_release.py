import importlib.util
import json
from contextlib import redirect_stderr
from io import StringIO
from pathlib import Path
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "check_pages_release", Path(__file__).with_name("check-pages-release.py"))
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


def tags(sweep="0.5.2", stash="0.5.2", unpack="0.5.1"):
    entries = [
        f"sweep-sha refs/tags/sweep-v{sweep}",
        f"stash-sha refs/tags/stash-v{stash}",
        "v04-sha refs/tags/v0.4.0",
        "v05-sha refs/tags/v0.5.0",
        "v051-sha refs/tags/v0.5.1",
    ]
    if unpack != "0.5.1":
        entries.append(f"unpack-sha refs/tags/unpack-v{unpack}")
    return "\n".join(entries)


def page(sweep="0.5.2", stash="0.5.2", unpack="0.5.1"):
    return {"version": sweep,
            "versions": {"sweep": sweep, "stash": stash, "unpack": unpack}}


class PagesReleaseGateTests(unittest.TestCase):
    def test_all_latest_published_versions_are_allowed(self):
        found = release.check_page(page(), tags())
        self.assertEqual(found, {"sweep": "0.5.2", "stash": "0.5.2", "unpack": "0.5.1"})

    def test_shared_monorepo_tag_supplies_unpack_version(self):
        self.assertEqual(
            release.latest_published_versions(tags()),
            {"sweep": "0.5.2", "stash": "0.5.2", "unpack": "0.5.1"},
        )

    def test_newer_shared_monorepo_tag_does_not_advance_any_tool(self):
        for newer_shared in ("0.5.2", "0.5.3"):
            with self.subTest(shared_tag=newer_shared):
                listing = tags() + f"\nnew-sha refs/tags/v{newer_shared}\n"
                self.assertEqual(
                    release.latest_published_versions(listing),
                    {"sweep": "0.5.2", "stash": "0.5.2", "unpack": "0.5.1"},
                )
                with self.assertRaisesRegex(ValueError, "latest published tool versions"):
                    release.check_page(
                        page(sweep="0.5.3", stash="0.5.3", unpack="0.5.3"), listing)

    def test_unpublished_candidate_is_rejected_even_if_manifest_version_is_valid(self):
        with self.assertRaisesRegex(ValueError, "latest published tag is 0.5.2"):
            release.check_page(page(sweep="0.5.3", stash="0.5.3", unpack="0.5.3"), tags())

    def test_page_older_than_latest_published_version_is_rejected(self):
        listing = tags(sweep="0.5.3", stash="0.5.3", unpack="0.5.2")
        with self.assertRaisesRegex(ValueError, "page has '0.5.2'"):
            release.check_page(page(), listing)

    def test_selects_newest_semver_tag_not_lexicographic_maximum(self):
        listing = tags() + "\ns refs/tags/sweep-v0.5.10\n"
        self.assertEqual(release.latest_published_versions(listing)["sweep"], "0.5.10")

    def test_missing_tool_tag_fails_closed(self):
        listing = "s refs/tags/sweep-v0.5.2\nst refs/tags/stash-v0.5.2\n"
        with self.assertRaisesRegex(ValueError, "no published version tag.*unpack"):
            release.check_page(page(), listing)

    def test_missing_per_tool_version_fails(self):
        payload = {"version": "0.5.2", "versions": {"sweep": "0.5.2", "stash": "0.5.2"}}
        with self.assertRaisesRegex(ValueError, "unpack.*latest published tag"):
            release.check_page(payload, tags())

    def test_inline_transcript_must_match_the_hosted_transcript(self):
        payload = page()
        html = ('<script type="application/json" id="transcripts-inline">'
                + json.dumps(payload) + '</script>')
        release.check_page(payload, tags(), html)
        changed = html.replace('0.5.2', '0.5.3')
        with self.assertRaisesRegex(ValueError, "inline transcripts differ"):
            release.check_page(payload, tags(), changed)

    def test_missing_inline_copy_fails(self):
        with self.assertRaisesRegex(ValueError, "no inline transcript copy"):
            release.check_page(page(), tags(), "<html></html>")

    def test_legacy_version_must_match_sweep_tag(self):
        payload = page()
        payload["version"] = "0.5.3"
        with self.assertRaisesRegex(ValueError, "legacy page version"):
            release.check_page(payload, tags())

    def test_invalid_inline_json_fails_closed(self):
        html = ('<script type="application/json" id="transcripts-inline">'
                '{bad json}</script>')
        with self.assertRaisesRegex(ValueError, "invalid JSON"):
            release.check_page(page(), tags(), html)

    def test_non_object_transcripts_root_fails_closed(self):
        with self.assertRaisesRegex(ValueError, "root must be an object"):
            release.check_page([], tags())

    def test_remote_tag_lookup_error_returns_failure(self):
        error = release.subprocess.CalledProcessError(128, ["git", "ls-remote"], stderr="offline")
        stderr = StringIO()
        with patch.object(release.subprocess, "run", side_effect=error), redirect_stderr(stderr):
            self.assertEqual(release.main(), 1)
        self.assertIn("Pages release gate", stderr.getvalue())
        self.assertIn("offline", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
