"""Fault cases for the release gate; no network or user files are used."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "check_release", Path(__file__).with_name("check-release.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseGate(unittest.TestCase):
    def setUp(self):
        self.expected = {tool: f"{tool}-v0.5.3" for tool in release.TOOLS}
        self.readme = "\n".join(
            f"cargo install --locked --git {release.REPO} --tag {tag} {tool}-cli"
            for tool, tag in self.expected.items())

    def test_all_documented_pins_are_required(self):
        self.assertEqual(release.pins(self.readme), self.expected)
        with self.assertRaisesRegex(RuntimeError, "unpack"):
            release.pins(self.readme.replace("unpack-cli", "other-cli"))

    def test_matching_version_strings_do_not_prove_a_tag_exists(self):
        listing = "\n".join(f"{'a' * 40}\trefs/tags/{tag}"
                            for tool, tag in self.expected.items() if tool != "unpack")
        with self.assertRaisesRegex(RuntimeError, "unpublished tags: unpack-v0.5.3"):
            release.check_tags(release.pins(self.readme), listing)
        release.check_tags(self.expected, listing + "\n" + "a" * 40
                           + "\trefs/tags/unpack-v0.5.3")

    def test_a_similarly_named_tag_does_not_satisfy_the_pin(self):
        listing = "\n".join(f"{'a' * 40}\trefs/tags/{tag}-old"
                            for tag in self.expected.values())
        with self.assertRaisesRegex(RuntimeError, "unpublished tags"):
            release.check_tags(self.expected, listing)

    def test_a_lightweight_release_tag_fails(self):
        annotated = "\n".join(f"{'a' * 40}\trefs/tags/{tag}\n{'b' * 40}\trefs/tags/{tag}^{{}}"
                               for tag in self.expected.values())
        release.check_annotated(self.expected, annotated)
        light = annotated.replace(f"refs/tags/stash-v0.5.3^{{}}", "refs/tags/other^{}")
        with self.assertRaisesRegex(RuntimeError, "lightweight release tags.*stash-v0.5.3"):
            release.check_annotated(self.expected, light)

    def test_older_lightweight_tags_do_not_fail(self):
        # Only the pinned tags are checked; an older lightweight tag is not.
        listing = "\n".join(f"{'a' * 40}\trefs/tags/{tag}\n{'b' * 40}\trefs/tags/{tag}^{{}}"
                             for tag in self.expected.values())
        listing += f"\n{'c' * 40}\trefs/tags/stash-v0.5.2"
        release.check_annotated(self.expected, listing)

    def test_wrong_installed_version_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(release, "run", return_value="sweep 0.5.2\n"):
                with self.assertRaisesRegex(RuntimeError, "unexpected version"):
                    release.smoke("sweep", "unused", Path(directory), {}, "0.5.3")

    def test_zero_exit_without_moving_files_is_not_success(self):
        for tool in ("sweep", "stash"):
            with self.subTest(tool=tool), tempfile.TemporaryDirectory() as directory:
                outputs = [f"{tool} 0.5.3\n", "{}", ""] if tool == "sweep" else [f"{tool} 0.5.3\n", ""]
                with patch.object(release, "run", side_effect=outputs):
                    with self.assertRaisesRegex(RuntimeError, "success without"):
                        release.smoke(tool, "unused", Path(directory), {}, "0.5.3")


if __name__ == "__main__":
    unittest.main()
