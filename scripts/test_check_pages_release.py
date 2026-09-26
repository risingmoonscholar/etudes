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


def release_row(tag_name, draft=False, prerelease=False):
    return {"tag_name": tag_name, "draft": draft, "prerelease": prerelease}


def tags(sweep="0.5.2", stash="0.5.2", unpack="0.5.1"):
    entries = [
        release_row(f"sweep-v{sweep}"),
        release_row(f"stash-v{stash}"),
        release_row("v0.4.0"),
        release_row("v0.5.0"),
        release_row("v0.5.1"),
    ]
    if unpack != "0.5.1":
        entries.append(release_row(f"unpack-v{unpack}"))
    return entries


def page(sweep="0.5.2", stash="0.5.2", unpack="0.5.1"):
    return {"version": sweep,
            "versions": {"sweep": sweep, "stash": stash, "unpack": unpack}}


WORKFLOW = Path(__file__).parents[1] / ".github/workflows/pages.yml"


def workflow_lines():
    return WORKFLOW.read_text().splitlines()


def named_steps():
    lines = workflow_lines()
    steps_start = lines.index("    steps:") + 1
    steps = []
    for line in lines[steps_start:]:
        if line.startswith("      - "):
            steps.append([line])
        elif steps and (line.startswith("        ") or not line.strip()):
            steps[-1].append(line)
        elif line and not line.startswith("      "):
            break
    return steps


def step_containing(steps, marker):
    for step in steps:
        if any(marker in line for line in step):
            return "\n".join(step)
    raise AssertionError(f"Pages workflow has no step containing {marker!r}")


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
                listing = tags() + [release_row(f"v{newer_shared}")]
                self.assertEqual(
                    release.latest_published_versions(listing),
                    {"sweep": "0.5.2", "stash": "0.5.2", "unpack": "0.5.1"},
                )
                with self.assertRaisesRegex(ValueError, "latest stable tool versions"):
                    release.check_page(
                        page(sweep="0.5.3", stash="0.5.3", unpack="0.5.3"), listing)

    def test_unpublished_candidate_is_rejected_even_if_manifest_version_is_valid(self):
        with self.assertRaisesRegex(ValueError, "latest stable release is 0.5.2"):
            release.check_page(page(sweep="0.5.3", stash="0.5.3", unpack="0.5.3"), tags())

    def test_page_older_than_latest_published_version_is_rejected(self):
        listing = tags(sweep="0.5.3", stash="0.5.3", unpack="0.5.2")
        with self.assertRaisesRegex(ValueError, "page has '0.5.2'"):
            release.check_page(page(), listing)

    def test_selects_newest_semver_tag_not_lexicographic_maximum(self):
        listing = tags() + [release_row("sweep-v0.5.10")]
        self.assertEqual(release.latest_published_versions(listing)["sweep"], "0.5.10")

    def test_missing_tool_stable_release_fails_closed(self):
        listing = [release_row("sweep-v0.5.2"), release_row("stash-v0.5.2")]
        with self.assertRaisesRegex(ValueError, "no stable published release.*unpack"):
            release.check_page(page(), listing)

    def test_drafts_and_prereleases_do_not_advance_stable_versions(self):
        listing = tags() + [
            release_row("sweep-v0.5.3", prerelease=True),
            release_row("stash-v0.5.3", draft=True),
            release_row("unpack-v0.5.3-rc.1"),
        ]
        self.assertEqual(
            release.latest_published_versions(listing),
            {"sweep": "0.5.2", "stash": "0.5.2", "unpack": "0.5.1"},
        )

    def test_release_without_explicit_stable_flags_is_ignored(self):
        listing = tags() + [{"tag_name": "sweep-v0.5.9", "draft": False}]
        self.assertEqual(release.latest_published_versions(listing)["sweep"], "0.5.2")

    def test_missing_per_tool_version_fails(self):
        payload = {"version": "0.5.2", "versions": {"sweep": "0.5.2", "stash": "0.5.2"}}
        with self.assertRaisesRegex(ValueError, "unpack.*latest stable release"):
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
        error = OSError("offline")
        stderr = StringIO()
        with patch.object(release, "fetch_published_releases", side_effect=error), redirect_stderr(stderr):
            self.assertEqual(release.main(), 1)
        self.assertIn("Pages release gate", stderr.getvalue())
        self.assertIn("offline", stderr.getvalue())

    def test_release_api_paginates_and_uses_github_token_when_available(self):
        class Response:
            def __init__(self, data):
                self.data = data

            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def read(self):
                return json.dumps(self.data).encode()

        first = [release_row(f"v0.4.{n}") for n in range(100)]
        second = [release_row("sweep-v0.5.2")]
        with patch.dict("os.environ", {"GH_TOKEN": "test-token"}, clear=False):
            with patch.object(release.urllib.request, "urlopen",
                              side_effect=[Response(first), Response(second)]) as open_url:
                self.assertEqual(release.fetch_published_releases(), first + second)
        self.assertIn("page=1", open_url.call_args_list[0].args[0].full_url)
        self.assertIn("page=2", open_url.call_args_list[1].args[0].full_url)
        self.assertEqual(open_url.call_args_list[0].args[0].get_header("Authorization"),
                         "Bearer test-token")
        request = open_url.call_args_list[0].args[0]
        self.assertEqual(request.get_header("Accept"), "application/vnd.github+json")
        self.assertEqual(request.get_header("X-github-api-version"), "2022-11-28")


class PagesReleaseWorkflowTests(unittest.TestCase):
    def test_deployment_is_triggered_by_stable_release_or_promotion(self):
        lines = workflow_lines()
        events = lines[lines.index("on:"):lines.index("permissions:")]
        significant = [line for line in events if line.strip() and not line.lstrip().startswith("#")]
        self.assertEqual(significant, ["on:", "  release:", "    types: [published, released]"])

    def test_live_tag_gate_runs_before_pages_artifact_upload(self):
        steps = named_steps()
        gate = next(i for i, step in enumerate(steps)
                    if any("Refuse unpublished or stale page versions" in line for line in step))
        upload = next(i for i, step in enumerate(steps)
                      if any("actions/upload-pages-artifact@v3" in line for line in step))
        self.assertLess(gate, upload)
        gate_step = "\n".join(steps[gate])
        self.assertIn("run: python3 scripts/check-pages-release.py", gate_step)
        self.assertNotIn("continue-on-error", gate_step)

    def test_deployment_checks_event_action_and_exact_per_tool_tag(self):
        step = step_containing(named_steps(), "Refuse invalid Pages release events")
        self.assertIn('if [ "$GITHUB_EVENT_NAME" != "release" ]; then', step)
        self.assertIn('"$GITHUB_EVENT_ACTION" != "published"', step)
        self.assertIn('"$GITHUB_EVENT_ACTION" != "released"', step)
        self.assertIn(r"^refs/tags/(sweep|stash|unpack)-v[0-9]+\.[0-9]+\.[0-9]+$", step)
        self.assertIn("exit 1", step)

    def test_checkout_is_pinned_to_the_release_tag_tree(self):
        checkout = step_containing(named_steps(), "actions/checkout@v4")
        self.assertIn("ref: ${{ github.sha }}", checkout)
        self.assertIn("commit GitHub associated with this release event", checkout)
        self.assertNotIn("ref: main", checkout)

    def test_prereleases_do_not_deploy_until_promoted(self):
        lines = workflow_lines()
        start = lines.index("  deploy:")
        end = next(i for i in range(start + 1, len(lines))
                   if lines[i].startswith("    runs-on:"))
        job = lines[start:end]
        significant = [line.strip() for line in job if line.strip() and not line.lstrip().startswith("#")]
        self.assertIn("github.event.release.prerelease == false", " ".join(significant))


if __name__ == "__main__":
    unittest.main()
