#!/usr/bin/env python3
"""Capture a tree's observable filesystem state for stress assertions.

The format is canonical JSON so names containing newlines remain unambiguous.
It intentionally uses lstat and never follows symlinks.
"""
from __future__ import annotations

import hashlib
import json
import os
import stat
import sys
from pathlib import Path


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(block)
    return hasher.hexdigest()


def entry(root: Path, path: Path) -> dict[str, object]:
    info = path.lstat()
    item: dict[str, object] = {"path": str(path.relative_to(root)), "mode": stat.S_IMODE(info.st_mode)}
    if stat.S_ISREG(info.st_mode):
        item.update(kind="file", size=info.st_size, sha256=digest(path))
    elif stat.S_ISDIR(info.st_mode):
        item.update(kind="directory")
    elif stat.S_ISLNK(info.st_mode):
        item.update(kind="symlink", target=os.readlink(path))
    else:
        item.update(kind="other")
    return item


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: snapshot.py DIRECTORY", file=sys.stderr)
        return 2
    root = Path(sys.argv[1]).resolve()
    if not root.is_dir():
        print(f"not a directory: {root}", file=sys.stderr)
        return 2
    entries = [entry(root, root)]
    for base, dirs, files in os.walk(root, topdown=True, followlinks=False):
        current = Path(base)
        names = sorted([*dirs, *files])
        for name in names:
            path = current / name
            entries.append(entry(root, path))
            if path.is_symlink() and name in dirs:
                dirs.remove(name)
    entries.sort(key=lambda value: str(value["path"]).encode("utf-8", "surrogateescape"))
    json.dump({"root": str(root), "entries": entries}, sys.stdout, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
