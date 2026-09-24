#!/usr/bin/env python3
"""Refuse to publish a Pages artifact newer than the latest public tool tags."""
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
REPO = "https://github.com/risingmoonscholar/etudes"
TOOLS = ("sweep", "stash", "unpack")
TAG = re.compile(r"^refs/tags/(sweep|stash|unpack)-v(\d+)\.(\d+)\.(\d+)$")


def latest_published_versions(tag_listing):
    versions = {tool: [] for tool in TOOLS}
    for line in tag_listing.splitlines():
        fields = line.split()
        if len(fields) != 2:
            continue
        match = TAG.fullmatch(fields[1])
        if match:
            tool, *parts = match.groups()
            versions[tool].append((tuple(map(int, parts)), ".".join(parts)))
    missing = [tool for tool in TOOLS if not versions[tool]]
    if missing:
        raise ValueError("no published version tag found for: " + ", ".join(missing))
    return {tool: max(versions[tool])[1] for tool in TOOLS}


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


def check_page(payload, tag_listing, html=None):
    if not isinstance(payload, dict):
        raise ValueError("transcripts.json root must be an object")
    if html is not None:
        check_inline_copy(payload, html)
    published = latest_published_versions(tag_listing)
    versions = payload.get("versions")
    if not isinstance(versions, dict):
        raise ValueError("transcripts.json has no per-tool versions")
    mismatches = [
        f"{tool}: page has {versions.get(tool)!r}, latest published tag is {version}"
        for tool, version in published.items()
        if versions.get(tool) != version
    ]
    if payload.get("version") != published["sweep"]:
        mismatches.append(
            f"legacy page version is {payload.get('version')!r}, "
            f"latest published sweep version is {published['sweep']}"
        )
    if mismatches:
        raise ValueError("page does not match the latest published tool versions:\n  "
                         + "\n  ".join(mismatches))
    return published


def main():
    try:
        result = subprocess.run(
            ["git", "ls-remote", "--tags", "--refs", REPO],
            cwd=ROOT, text=True, capture_output=True, check=True,
        )
        payload = json.loads((ROOT / "demo/transcripts.json").read_text())
        html = (ROOT / "demo/index.html").read_text()
        published = check_page(payload, result.stdout, html)
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or error.stdout or str(error)).strip()
        print(f"FAIL Pages release gate: remote tag lookup failed: {detail}",
              file=sys.stderr)
        return 1
    except (OSError, json.JSONDecodeError, ValueError) as error:
        print(f"FAIL Pages release gate: {error}", file=sys.stderr)
        return 1
    print("PASS Pages release gate: page matches latest published versions "
          + ", ".join(f"{tool} {version}" for tool, version in published.items()))
    return 0


if __name__ == "__main__":
    sys.exit(main())
