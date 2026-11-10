#!/usr/bin/env python3
"""Run one release's before/after commands on real builds and write its entry.

    scripts/release-runs.py TOOL VERSION --before REF --after REF
                            [--logs DIR] [--target-dir DIR]
    scripts/release-runs.py TOOL VERSION --cuts-only

--cuts-only builds and runs nothing: it writes the entry again from the raw
logs already in site/changelog/logs/, after a change to the fixture's show
lines or result labels. It refuses when the fixture's setup or commands
changed since the runs.

Example, at tag time:

    scripts/release-runs.py sweep 0.5.5 --before sweep-v0.5.4 --after sweep-v0.5.5

What it does:
  1. Builds TOOL from each ref out of tree: a detached git worktree in a temp
     folder, `cargo build --release --locked -p TOOL-cli`, the binary copied
     out, the worktree removed.
  2. For each build, makes a fresh scratch folder, runs the fixture's setup
     there, then runs the fixture's commands in it, one after the other. Every
     command runs with a fake HOME, a throwaway ETUDE_STATE_DIR, a random
     64-hex ETUDE_JOURNAL_KEY and stdin closed. stdout and stderr go to one
     pipe, so the log holds them in the order the tool printed them.
  3. After the "after" commands, runs the fixture's next command on the after
     build in the same folder.
  4. Writes the raw output of every command, byte for byte, and record.json
     (commands, exit codes, sha256 of each log and each binary, commits, the
     folder listing) to --logs, by default site/changelog/logs/TOOL-VERSION/.
  5. Writes site/changelog/TOOL-VERSION.json from the record and the fixture.

The fixture is site/changelog/TOOL-VERSION.fixture (JSON). It holds the shell
lines that make the test folder, the commands, which whole lines of each
output the page shows (regular expressions, matched in order, one line each),
and how to list the result. See site/changelog/sweep-0.5.5.fixture.

The hand-written fields of an existing entry are kept as they are: date,
label, headline, lead, next.note and film. Everything else in the entry is
derived from the record, by entry_from_record() below, which check-site.py
runs again to catch an edited output line, exit code or commit.

Standard library only. Exit 0 written, 1 a build or run failed, 2 bad usage.
Nothing is written to site/ unless both builds and every run finished.
"""
import argparse
import datetime
import hashlib
import json
import os
import re
import secrets
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CHANGELOG = ROOT / "site" / "changelog"
LOGS = CHANGELOG / "logs"
SIDES = ("before", "after")
HAND = ("date", "label", "headline", "lead")
ABOUT = ("One release entry in the release format, written by scripts/release-runs.py from "
         "real runs of two builds. The commands, output lines, exit codes, result files, "
         "builds and footer come from the raw record in site/changelog/logs/; whole lines "
         "are cut to fit by the fixture, no line is edited. Only date, label, headline, "
         "lead, next.note and film are written by hand.")
PATH = "/usr/bin:/bin:/usr/sbin:/sbin"


class RunError(Exception):
    pass


# --- shared with check-site.py ----------------------------------------------

def fixture_path(tool, version):
    return CHANGELOG / f"{tool}-{version}.fixture"


def load_fixture(path):
    try:
        fixture = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise RunError(f"{path} cannot be read: {error}") from error
    for key in ("folder", "describe", "setup", "runs", "result", "next"):
        if key not in fixture:
            raise RunError(f"{path} has no {key}")
    for run in fixture["runs"]:
        for side in SIDES:
            if side not in run.get("show", {}):
                raise RunError(f"{path}: the run {run.get('command')!r} has no show.{side}")
    return fixture


def runs_digest(fixture):
    """sha256 of what decides the runs: the setup and the commands, not the cuts."""
    basis = {"setup": fixture["setup"], "commands": [r["command"] for r in fixture["runs"]],
             "next": fixture["next"]["command"]}
    return hashlib.sha256(json.dumps(basis, sort_keys=True).encode()).hexdigest()


def log_lines(text):
    lines = text.split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    return lines


def cut(lines, patterns, where):
    """The whole lines PATTERNS pick, in order: each the next line it matches."""
    shown, at = [], 0
    for pattern in patterns:
        for i in range(at, len(lines)):
            if re.search(pattern, lines[i]):
                shown.append(lines[i])
                at = i + 1
                break
        else:
            raise RunError(f"{where}: the show pattern {pattern!r} matches no line after "
                           f"line {at} of the output")
    return shown


def build_label(side, build):
    """'tag sweep-v0.5.4 (commit de8f307)', or for an untagged after build
    'the 0.5.5 release candidate at commit a0ea3af, not tagged yet'."""
    if build.get("tag"):
        return f"tag {build['tag']} (commit {build['commit']})"
    if side == "before":
        return f"commit {build['commit']} ({build['version_output']})"
    version = build["version_output"].split()[-1]
    return f"the {version} release candidate at commit {build['commit']}, not tagged yet"


def build_name(side, build, version):
    if build.get("tag"):
        return f"tag {build['tag']}"
    return f"commit {build['commit']}" if side == "before" else f"the {version} release candidate"


def entry_from_record(record, fixture, hand, read_log):
    """The entry: hand-written fields from HAND, every other field from the record.

    READ_LOG(name) returns a log's text. check-site.py calls this again on the
    committed logs and compares, so the page cannot drift from the runs.
    """
    tool, version = record["tool"], record["version"]
    any_cut = False

    def shown(run, patterns, where):
        nonlocal any_cut
        lines = log_lines(read_log(run["log"]))
        picked = cut(lines, patterns, where)
        any_cut = any_cut or len(picked) < len(lines)
        return {"command": run["command"], "output": picked, "exit": run["exit"]}

    entry = {"_about": ABOUT, "tool": tool, "version": version}
    for key in HAND:
        entry[key] = hand.get(key, "" if key != "lead" else [])
    entry["folder"] = fixture["folder"]
    for side in SIDES:
        data = record["sides"][side]
        runs = [shown(run, spec["show"][side], f"{side} run {n + 1} ({run['command']})")
                for n, (run, spec) in enumerate(zip(data["runs"], fixture["runs"]))]
        entry[side] = {
            "build": record["builds"][side]["version_output"],
            "runs": runs,
            "result": {"label": fixture["result"][side]["label"], "files": data["result_files"]},
            "exit": runs[-1]["exit"],
        }
    nxt = shown(record["next"], fixture["next"]["show"], f"next ({record['next']['command']})")
    entry["next"] = {"note": hand.get("next_note", []), **nxt}
    if hand.get("film"):
        entry["film"] = hand["film"]
    builds = {}
    for side in SIDES:
        b = record["builds"][side]
        builds[side] = {"name": build_name(side, b, version), "commit": b["commit"], "rev": b["rev"], "sha256": b["sha256"]}
    builds["after"]["tagged"] = bool(record["builds"]["after"].get("tag"))
    entry["builds"] = builds
    labels = [build_label(s, record["builds"][s]) for s in SIDES]
    entry["footer"] = (
        f"Both outputs come from real runs of two builds: {labels[0]} and {labels[1]}. "
        f"Each ran the same commands on a fresh copy of one {fixture.get('kind', 'synthetic folder')}, "
        f"{fixture['folder']}: {fixture['describe']}. "
        + ("Output is shortened to fit by cutting whole lines. " if any_cut else "")
        + "Nothing outside the scratch folder was touched.")
    return entry


def hand_fields(entry):
    hand = {key: entry[key] for key in HAND if key in entry}
    if isinstance(entry.get("next"), dict) and "note" in entry["next"]:
        hand["next_note"] = entry["next"]["note"]
    if entry.get("film"):
        hand["film"] = entry["film"]
    return hand


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


# --- git and cargo -----------------------------------------------------------

def git(*args, check=True):
    done = subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True)
    if check and done.returncode != 0:
        raise RunError(f"git {' '.join(args)} failed: {done.stderr.strip()}")
    return done.stdout.strip() if done.returncode == 0 else None


def resolve(ref):
    rev = git("rev-parse", "--verify", "--quiet", f"{ref}^{{commit}}", check=False)
    if not rev:
        raise RunError(f"{ref} is not a commit or tag in this repository. Run git fetch --tags, "
                       "then try again.")
    return rev


def tag_at(name, rev):
    """NAME if it is a tag that points at REV."""
    tagged = git("rev-parse", "--verify", "--quiet", f"refs/tags/{name}^{{commit}}", check=False)
    return name if tagged == rev else None


def build(tool, ref, rev, tmp, target, side):
    src = tmp / f"src-{side}"
    git("worktree", "add", "--detach", "--quiet", str(src), rev)
    try:
        env = dict(os.environ, CARGO_TARGET_DIR=str(target))
        print(f"..   building {tool} at {ref} ({rev[:7]})", file=sys.stderr)
        log = tmp / f"build-{side}.log"
        with open(log, "w") as out:
            done = subprocess.run(["cargo", "build", "--release", "--locked", "-p", f"{tool}-cli"],
                                  cwd=src, env=env, stdout=out, stderr=subprocess.STDOUT,
                                  stdin=subprocess.DEVNULL)
        if done.returncode != 0:
            tail = "\n".join(log.read_text(errors="replace").splitlines()[-20:])
            raise RunError(f"cargo build of {tool} at {ref} failed (exit {done.returncode}):\n{tail}")
        binary = tmp / f"bin-{side}" / tool
        binary.parent.mkdir()
        shutil.copy2(target / "release" / tool, binary)
        return binary
    finally:
        git("worktree", "remove", "--force", str(src), check=False)
        git("worktree", "prune", check=False)


# --- runs --------------------------------------------------------------------

def listing(folder):
    out = []
    for path in sorted(folder.rglob("*"), key=lambda p: p.relative_to(folder).as_posix()):
        rel = path.relative_to(folder).as_posix()
        if path.is_symlink():
            out.append(f"{rel} -> {os.readlink(path)}")
        elif path.is_dir():
            out.append(rel + "/")
        else:
            out.append(rel)
    return out


def result_files(work, spec, before):
    names = []
    for path in sorted(work.glob(spec["glob"]), key=lambda p: p.name):
        if path.is_dir() and not path.is_symlink():
            continue
        rel = path.relative_to(work).as_posix()
        if spec.get("new") and rel in before:
            continue
        names.append(f"{path.name} -> {os.readlink(path)}" if path.is_symlink() else path.name)
    return names


class Scratch:
    """One fresh folder per build: work/ for the fixture, home/, state/, tmp/."""

    def __init__(self, base, side):
        self.root = base / f"run-{side}"
        self.work = self.root / "work"
        for name in ("work", "home", "state", "tmp"):
            (self.root / name).mkdir(parents=True)
        self.env = {"PATH": PATH, "HOME": str(self.root / "home"),
                    "ETUDE_STATE_DIR": str(self.root / "state"),
                    "ETUDE_JOURNAL_KEY": secrets.token_hex(32),
                    "TMPDIR": str(self.root / "tmp") + "/", "LANG": "en_US.UTF-8"}

    def setup(self, fixture):
        script = self.root / "setup.sh"
        script.write_text("\n".join(fixture["setup"]) + "\n", encoding="utf-8")
        done = subprocess.run(["/bin/sh", "-eu", str(script)], cwd=self.work, env=self.env,
                              capture_output=True, text=True, stdin=subprocess.DEVNULL)
        if done.returncode != 0:
            raise RunError(f"the fixture setup failed (exit {done.returncode}): "
                           f"{(done.stdout + done.stderr).strip()}")

    def run(self, tool, binary, command, logs, name):
        argv = shlex.split(command)
        if not argv or argv[0] != tool:
            raise RunError(f"the fixture command {command!r} does not start with {tool}")
        done = subprocess.run([str(binary), *argv[1:]], cwd=self.work, env=self.env,
                              stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT, timeout=300)
        try:
            done.stdout.decode("utf-8")
        except UnicodeDecodeError as error:
            raise RunError(f"{command!r} printed output that is not UTF-8: {error}") from error
        (logs / name).write_bytes(done.stdout)
        return {"command": command, "exit": done.returncode, "log": name,
                "sha256": hashlib.sha256(done.stdout).hexdigest()}


def run_side(tool, side, binary, fixture, base, logs):
    scratch = Scratch(base, side)
    scratch.setup(fixture)
    before = listing(scratch.work)
    runs = [scratch.run(tool, binary, spec["command"], logs, f"{side}-{n + 1}.txt")
            for n, spec in enumerate(fixture["runs"])]
    data = {"listing_before": before, "runs": runs,
            "result_files": result_files(scratch.work, fixture["result"][side],
                                         {p.split(" -> ")[0].rstrip("/") for p in before})}
    nxt = None
    if side == "after":
        nxt = scratch.run(tool, binary, fixture["next"]["command"], logs, "next.txt")
    data["listing_after"] = listing(scratch.work)
    return data, nxt


def version_of(binary, base):
    home = base / "version-home"
    home.mkdir(exist_ok=True)
    done = subprocess.run([str(binary), "--version"], capture_output=True, text=True,
                          env={"PATH": PATH, "HOME": str(home)}, stdin=subprocess.DEVNULL)
    if done.returncode != 0:
        raise RunError(f"{binary.name} --version exited {done.returncode}")
    return done.stdout.strip()


def commit_date(rev):
    return git("show", "-s", "--format=%cs", rev)


def cuts_only(tool, version, entry_path, logs):
    try:
        fixture = load_fixture(fixture_path(tool, version))
        record = json.loads((logs / "record.json").read_text(encoding="utf-8"))
        if record.get("runs_sha256") != runs_digest(fixture):
            raise RunError("the fixture's setup or commands changed since the runs; run the builds again "
                           "with --before and --after")
        for run in [r for side in SIDES for r in record["sides"][side]["runs"]] + [record["next"]]:
            if sha256_file(logs / run["log"]) != run["sha256"]:
                raise RunError(f"{logs / run['log']} differs from its sha256 in record.json")
        old = json.loads(entry_path.read_text(encoding="utf-8")) if entry_path.is_file() else {}
        entry = entry_from_record(record, fixture, hand_fields(old),
                                  lambda n: (logs / n).read_text(encoding="utf-8"))
    except (RunError, OSError, json.JSONDecodeError) as error:
        print(f"FAIL {error}", file=sys.stderr)
        return 1
    entry_path.write_text(json.dumps(entry, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"ok   wrote {entry_path.relative_to(ROOT)} from the raw logs in {logs}")
    return 0


def main(argv):
    parser = argparse.ArgumentParser(
        description="Build two refs, run a release fixture on each, write the changelog entry.")
    parser.add_argument("tool")
    parser.add_argument("version")
    parser.add_argument("--before", help="ref of the previous release (a tag)")
    parser.add_argument("--after", help="ref of this release: its tag, or a candidate commit")
    parser.add_argument("--cuts-only", action="store_true",
                        help="write the entry again from the committed raw logs; build and run nothing")
    parser.add_argument("--logs", help="where the raw logs and record.json go "
                        "(default site/changelog/logs/TOOL-VERSION/)")
    parser.add_argument("--target-dir", help="CARGO_TARGET_DIR to build in and keep "
                        "(default: a temp folder, removed after)")
    args = parser.parse_args(argv[1:])
    if not re.fullmatch(r"[a-z]+", args.tool) or not re.fullmatch(r"\d+\.\d+\.\d+", args.version):
        parser.error("TOOL is a tool name such as sweep, VERSION is X.Y.Z")
    name = f"{args.tool}-{args.version}"
    entry_path = CHANGELOG / f"{name}.json"
    logs = Path(args.logs).resolve() if args.logs else LOGS / name
    if args.cuts_only:
        return cuts_only(args.tool, args.version, entry_path, logs)
    if not args.before or not args.after:
        parser.error("--before and --after are required (or --cuts-only)")

    try:
        fixture = load_fixture(fixture_path(args.tool, args.version))
        old = {}
        if entry_path.is_file():
            old = json.loads(entry_path.read_text(encoding="utf-8"))
        revs = {"before": resolve(args.before), "after": resolve(args.after)}
        tmp = Path(tempfile.mkdtemp(prefix="etudes-release-runs-"))
        try:
            target = Path(args.target_dir).resolve() if args.target_dir else tmp / "target"
            staged = tmp / "logs"
            staged.mkdir()
            record = {"tool": args.tool, "version": args.version,
                      "fixture": fixture_path(args.tool, args.version).name,
                      "runs_sha256": runs_digest(fixture),
                      "env": ["PATH=" + PATH, "HOME=<scratch>/home", "ETUDE_STATE_DIR=<scratch>/state",
                              "ETUDE_JOURNAL_KEY=<64 random hex, not recorded>",
                              "TMPDIR=<scratch>/tmp/", "LANG=en_US.UTF-8", "stdin closed"],
                      "builds": {}, "sides": {}}
            refs = {"before": args.before, "after": args.after}
            for side in SIDES:
                binary = build(args.tool, refs[side], revs[side], tmp, target, side)
                version_line = version_of(binary, tmp)
                tag = (tag_at(args.before, revs[side]) if side == "before"
                       else tag_at(f"{args.tool}-v{args.version}", revs[side]))
                record["builds"][side] = {
                    "ref": refs[side], "rev": revs[side], "commit": revs[side][:7], "tag": tag,
                    "build": f"cargo build --release --locked -p {args.tool}-cli",
                    "version_output": version_line, "sha256": sha256_file(binary)}
                if side == "after" and version_line != f"{args.tool} {args.version}":
                    raise RunError(f"the after build ({args.after}) says {version_line!r}, "
                                   f"not '{args.tool} {args.version}'. Is that the right ref?")
                print(f"..   running the fixture on {version_line}", file=sys.stderr)
                record["sides"][side], nxt = run_side(args.tool, side, binary, fixture, tmp, staged)
                if nxt:
                    record["next"] = nxt
            hand = hand_fields(old)
            hand.setdefault("date", commit_date(revs["after"]))
            entry = entry_from_record(record, fixture, hand,
                                      lambda n: (staged / n).read_text(encoding="utf-8"))
            (staged / "record.json").write_text(json.dumps(record, indent=2, ensure_ascii=False) + "\n",
                                                encoding="utf-8")
            # Replace the old logs only now that everything ran.
            if logs.exists():
                ours = re.compile(r"^(record\.json|next\.txt|(before|after)-\d+\.txt)$")
                strangers = [p.name for p in logs.iterdir() if not ours.match(p.name)]
                if strangers:
                    raise RunError(f"{logs} holds files this script did not write "
                                   f"({', '.join(sorted(strangers))}); nothing was replaced")
                shutil.rmtree(logs)
            logs.parent.mkdir(parents=True, exist_ok=True)
            shutil.copytree(staged, logs)
            entry_path.write_text(json.dumps(entry, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        finally:
            shutil.rmtree(tmp, ignore_errors=True)
            git("worktree", "prune", check=False)
    except RunError as error:
        print(f"FAIL {error}", file=sys.stderr)
        return 1
    missing = [k for k in ("headline", "label", "lead") if not entry.get(k)]
    if not entry["next"]["note"]:
        missing.append("next.note")
    print(f"ok   wrote {entry_path.relative_to(ROOT)} and the raw logs in {logs}")
    for side in SIDES:
        b = record["builds"][side]
        print(f"ok   {side}: {b['version_output']} at {b['commit']}"
              f"{' (tag ' + b['tag'] + ')' if b['tag'] else ', not tagged'}, "
              f"exit codes {[r['exit'] for r in record['sides'][side]['runs']]}")
    if missing:
        print(f"note write {', '.join(missing)} by hand in {entry_path.relative_to(ROOT)}; "
              "the site does not build without them")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
