#!/usr/bin/env python3
"""Install and exercise candidates or the exact public tags advertised in README.

--candidate builds this checkout; it is not evidence of publication or review.
--published first checks remote tags, then installs from those tags into a fresh
temporary root. No user installation or real folder is changed.
"""
import argparse
import json
import os
from pathlib import Path
import re
import secrets
import subprocess
import sys
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
REPO = "https://github.com/risingmoonscholar/etudes"
TOOLS = ("sweep", "stash", "unpack")


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def pins(readme):
    result = {}
    for tool in TOOLS:
        matches = re.findall(
            rf"^cargo install --locked --git {re.escape(REPO)} --tag "
            rf"({tool}-v\d+\.\d+\.\d+) {tool}-cli$", readme, re.M)
        require(len(matches) == 1, f"expected one locked tagged install for {tool}")
        result[tool] = matches[0]
    return result


def check_tags(expected, listing):
    refs = {line.split()[1] for line in listing.splitlines() if len(line.split()) == 2}
    missing = [tag for tag in expected.values() if f"refs/tags/{tag}" not in refs]
    require(not missing, "unpublished tags: " + ", ".join(missing))


def run(argv, env=None, expected=0):
    result = subprocess.run([str(a) for a in argv], cwd=ROOT, env=env,
                            text=True, capture_output=True)
    require(result.returncode == expected,
            f"{argv[0]} exited {result.returncode}, expected {expected}\n"
            + result.stdout + result.stderr)
    return result.stdout


def smoke(tool, binary, work, env, version):
    reported = run([binary, "--version"], env).strip()
    require(reported == f"{tool} {version}", f"unexpected version: {reported}")
    folder = work / tool
    folder.mkdir()
    originals = {f"note-{n}.txt": f"synthetic fixture {n}\n" for n in range(3)}
    for name, content in originals.items():
        path = folder / name
        path.write_text(content)
        os.utime(path, (1_600_000_000, 1_600_000_000))
    if tool == "sweep":
        json.loads(run([binary, folder, "--json"], env))
        run([binary, "apply", folder, "--yes"], env)
        require(not any((folder / name).exists() for name in originals),
                "sweep reported success without moving the fixture")
        run([binary, "undo", folder], env)
    elif tool == "stash":
        run([binary, folder, "--for", "3d"], env)
        require(not any((folder / name).exists() for name in originals),
                "stash reported success without holding the fixture")
        run([binary, "pop", folder], env)
    else:
        archive = folder / "fixture.zip"
        member = folder / "ordinary.txt"
        member.write_text("synthetic archive\n")
        # Use an actual macOS archive writer, as the product's existing ZIP
        # integration tests do. This preserves regular-file mode information.
        run(["/usr/bin/zip", "-j", archive, member], env)
        destination = folder / "extracted"
        run([binary, archive, "--into", destination], env)
        require((destination / "ordinary.txt").read_text() == "synthetic archive\n",
                "extracted bytes differ")
        run([binary, archive, "--into", destination], env, expected=2)
    for name, content in originals.items():
        require((folder / name).read_text() == content, f"{tool}: original not restored: {name}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--candidate", action="store_true")
    mode.add_argument("--published", action="store_true")
    parser.add_argument("--tags-only", action="store_true",
                        help="with --published: verify tag existence only, not installation")
    args = parser.parse_args()
    if args.tags_only and not args.published:
        parser.error("--tags-only requires --published")
    expected = pins((ROOT / "README.md").read_text())
    versions = {}
    for tool in TOOLS:
        manifest = tomllib.loads((ROOT / f"crates/{tool}-cli/Cargo.toml").read_text())
        versions[tool] = manifest["package"]["version"]
        require(expected[tool] == f"{tool}-v{versions[tool]}", f"{tool}: manifest/pin mismatch")
    if args.published:
        check_tags(expected, run(["git", "ls-remote", "--tags", "--refs", REPO]))
        if args.tags_only:
            print("PASS public tags exist; installation and review not checked")
            return
    revision = run(["git", "rev-parse", "HEAD"]).strip()
    print(f"mode={'candidate' if args.candidate else 'published'} checkout={revision}", flush=True)
    with tempfile.TemporaryDirectory(prefix="etudes-release-check-") as temporary:
        work = Path(temporary)
        env = dict(os.environ, ETUDE_STATE_DIR=str(work / "state"),
                   ETUDE_JOURNAL_KEY=secrets.token_hex(32))
        for tool in TOOLS:
            install = work / f"install-{tool}"
            argv = ["cargo", "install", "--locked", "--root", install]
            if args.candidate:
                argv += ["--path", ROOT / f"crates/{tool}-cli"]
            else:
                argv += ["--git", REPO, "--tag", expected[tool], f"{tool}-cli"]
            print(f"installing {tool} {versions[tool]}", flush=True)
            run(argv)
            smoke(tool, install / "bin" / tool, work, env, versions[tool])
            print(f"PASS {tool}: install, version, synthetic operation and recovery/refusal", flush=True)
    print("PASS installation checks; independent review not assessed")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError) as error:
        print(f"FAIL {error}", file=sys.stderr)
        sys.exit(1)
