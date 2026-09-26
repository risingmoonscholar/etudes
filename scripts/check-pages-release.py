#!/usr/bin/env python3
"""Refuse to publish a Pages artifact newer than the latest stable tool releases."""
import json
import os
from pathlib import Path
import re
import sys
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
RELEASES_API = "https://api.github.com/repos/risingmoonscholar/etudes/releases"
TOOLS = ("sweep", "stash", "unpack")
TAG = re.compile(r"^refs/tags/(?:(sweep|stash|unpack)-v|v)(\d+)\.(\d+)\.(\d+)$")
# v0.5.1 is the last confirmed shared workspace release. Later releases use
# per-tool tags; a new generic tag must not silently advance a tool that was
# omitted from that release.
LEGACY_SHARED_TAG_CUTOFF = (0, 5, 1)


def latest_published_versions(releases):
    versions = {tool: [] for tool in TOOLS}
    if not isinstance(releases, list):
        raise ValueError("GitHub releases response must be a list")
    for release in releases:
        if not isinstance(release, dict):
            continue
        # GitHub supplies both booleans on every Release object. Missing or
        # malformed values must not make an unclassified release publishable.
        if release.get("draft") is not False or release.get("prerelease") is not False:
            continue
        tag_name = release.get("tag_name")
        if not isinstance(tag_name, str):
            continue
        match = TAG.fullmatch(f"refs/tags/{tag_name}")
        if match:
            tool, *parts = match.groups()
            version = ".".join(parts)
            entry = (tuple(map(int, parts)), version)
            if tool is None and tuple(map(int, parts)) <= LEGACY_SHARED_TAG_CUTOFF:
                # Historical monorepo release tags apply to every workspace
                # tool only through the confirmed transition point.
                for release_tool in TOOLS:
                    versions[release_tool].append(entry)
            elif tool is not None:
                versions[tool].append(entry)
    missing = [tool for tool in TOOLS if not versions[tool]]
    if missing:
        raise ValueError("no stable published release found for: " + ", ".join(missing))
    return {tool: max(versions[tool])[1] for tool in TOOLS}


def fetch_published_releases():
    """Read every public release, excluding drafts and prereleases in the gate."""
    releases = []
    page = 1
    while True:
        request = urllib.request.Request(
            f"{RELEASES_API}?per_page=100&page={page}",
            headers={
                "Accept": "application/vnd.github+json",
                "User-Agent": "etudes-pages-release-gate",
                "X-GitHub-Api-Version": "2022-11-28",
            },
        )
        token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
        if token:
            request.add_header("Authorization", f"Bearer {token}")
        with urllib.request.urlopen(request, timeout=20) as response:
            batch = json.loads(response.read())
        if not isinstance(batch, list):
            raise ValueError("GitHub releases response must be a list")
        releases.extend(batch)
        if len(batch) < 100:
            return releases
        page += 1


def check_inline_copy(payload, html):
    match = re.search(
        r'<script type="application/json" id="transcripts-inline">(.*?)</script>',
        html, re.DOTALL,
    )
    if not match:
        raise ValueError("demo/index.html has no inline transcript copy")
    try:
        inline = json.loads(match.group(1))
    except json.JSONDecodeError as error:
        raise ValueError(f"inline transcript copy is invalid JSON: {error}") from error
    if inline != payload:
        raise ValueError("inline transcripts differ from demo/transcripts.json")


def check_page(payload, releases, html=None):
    if not isinstance(payload, dict):
        raise ValueError("transcripts.json root must be an object")
    if html is not None:
        check_inline_copy(payload, html)
    published = latest_published_versions(releases)
    versions = payload.get("versions")
    if not isinstance(versions, dict):
        raise ValueError("transcripts.json has no per-tool versions")
    mismatches = [
        f"{tool}: page has {versions.get(tool)!r}, latest stable release is {version}"
        for tool, version in published.items()
        if versions.get(tool) != version
    ]
    if payload.get("version") != published["sweep"]:
        mismatches.append(
            f"legacy page version is {payload.get('version')!r}, "
            f"latest stable sweep release is {published['sweep']}"
        )
    if mismatches:
        raise ValueError("page does not match the latest stable tool versions:\n  "
                         + "\n  ".join(mismatches))
    return published


def main():
    try:
        payload = json.loads((ROOT / "demo/transcripts.json").read_text())
        html = (ROOT / "demo/index.html").read_text()
        published = check_page(payload, fetch_published_releases(), html)
    except (OSError, json.JSONDecodeError, ValueError) as error:
        print(f"FAIL Pages release gate: stable release lookup or validation failed: {error}",
              file=sys.stderr)
        return 1
    print("PASS Pages release gate: page matches latest stable release versions "
          + ", ".join(f"{tool} {version}" for tool, version in published.items()))
    return 0


if __name__ == "__main__":
    sys.exit(main())
