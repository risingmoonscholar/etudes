#!/usr/bin/env python3
"""Run one release's before/after commands on real builds and write its entry.

    scripts/release-runs.py TOOL VERSION --before REF --after REF
                            [--logs DIR] [--target-dir DIR]
    scripts/release-runs.py TOOL VERSION --cuts-only

Example, at tag time:

    scripts/release-runs.py sweep 0.5.5 --before sweep-v0.5.4 --after sweep-v0.5.5

What it does:
  1. Refuses, before it builds anything, a ref whose sweep or stash source
     does not read ETUDE_JOURNAL_KEY. Those builds fall back to the login
     keychain through /usr/bin/security, and no run here may reach it.
  2. Builds TOOL from each ref out of tree: a detached git worktree in a temp
     folder, `cargo build --release --locked -p TOOL-cli`, the binary copied
     out, that worktree removed.
  3. For each build, makes a fresh scratch folder, runs the fixture's setup
     there, then the fixture's commands, one after the other (a run marked
     "fresh" gets a new copy of the folder). Every command runs:
       - inside sandbox-exec, with a profile that denies any file write
         outside the scratch folder and any exec of /usr/bin/security;
       - with a `security` on the front of PATH that only refuses;
       - with a fake HOME, a throwaway ETUDE_STATE_DIR, a random 64-hex
         ETUDE_JOURNAL_KEY and stdin closed.
     stdout and stderr go to one pipe, so a log holds them in the order the
     tool printed them.
  4. After the "after" commands, runs the fixture's next command on the after
     build, in the same folder.
  5. Writes each command's output to --logs (default
     site/changelog/logs/TOOL-VERSION/) with one change: the scratch folder's
     path, which differs on every run and machine, is written as <scratch>.
     Nothing else in a log is changed. record.json holds the commands, exit
     codes, sha256 of each log and binary, commits and folder listings.
  6. Writes site/changelog/TOOL-VERSION.json from the record and the fixture.

--cuts-only builds and runs nothing: it writes the entry again from the logs
already in site/changelog/logs/, after a change to the fixture's show lines or
labels. It refuses when the fixture's setup, commands or result globs changed
since the runs.

The fixture is site/changelog/TOOL-VERSION.fixture (JSON): the shell lines that
make the test folder, the commands, which whole lines of each output the page
shows (regular expressions, matched in order, one line each), which JSON keys
of a --json run it reports, and how to list the result. See
site/changelog/sweep-0.5.5.fixture.

Hand-written fields of an existing entry are kept: label, headline, lead,
next.note and film. Everything else is derived from the record by
entry_from_record(), which check-site.py runs again to catch an edited output
line, exit code or commit. Whether the release is tagged, and its date, are
not stored: scripts/build-site.py reads them from git when it builds.

Standard library only. Exit 0 written, 1 refused or a build or run failed,
2 bad usage. Nothing is written to site/ unless both builds and every run
finished.
"""
import argparse
import hashlib
import json
import os
import re
import secrets
import shlex
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CHANGELOG = ROOT / "site" / "changelog"
LOGS = CHANGELOG / "logs"
SIDES = ("before", "after")
HAND = ("label", "headline", "lead")
ABOUT = ("One release entry in the release format, written by scripts/release-runs.py from "
         "real runs of two builds. The commands, output lines, exit codes, result files and "
         "builds come from the raw record in site/changelog/logs/; whole lines are cut to fit "
         "by the fixture, no line is edited. Only label, headline, lead, next.note and film "
         "are written by hand. Whether the release is tagged, and its date, come from git "
         "when the site is built.")
PATH = "/usr/bin:/bin:/usr/sbin:/sbin"
# The tools that keep an encrypted journal. Without ETUDE_JOURNAL_KEY support
# their builds call /usr/bin/security for a key in the login keychain.
KEY_TOOLS = ("sweep", "stash")
SECURITY = "/usr/bin/security"
SCRATCH = "<scratch>"
# A log or record that still holds one of these names a machine's own folder.
MACHINE_PATH = re.compile(r"/(private/)?var/folders/|/private/tmp/|/Users/|(^|[\s'\"=])/tmp/")
SANDBOX_NOTE = ("Every command ran in a sandbox that let it write only inside its scratch "
                "folder and could not run /usr/bin/security.")


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
    """sha256 of what decides the runs: the setup, the commands, which runs get a
    fresh folder, and which files the result lists. Not the cuts, the JSON keys
    or the labels, which only change the page."""
    basis = {"setup": fixture["setup"],
             "commands": [[r["command"], bool(r.get("fresh"))] for r in fixture["runs"]],
             "next": fixture["next"]["command"],
             "result": {side: {"glob": fixture["result"][side]["glob"],
                               "new": bool(fixture["result"][side].get("new"))} for side in SIDES}}
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


def json_fields(text, paths, where):
    """For a --json run: [{"path", "present", "value"}] for each dotted key path."""
    if not paths:
        return None
    docs = []
    for line in log_lines(text):
        try:
            doc = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(doc, dict):
            docs.append(doc)
    if len(docs) != 1:
        raise RunError(f"{where}: expected one JSON object line in the output, found {len(docs)}")
    fields = []
    for path in paths:
        node, present = docs[0], True
        for key in path.split("."):
            if isinstance(node, dict) and key in node:
                node = node[key]
            else:
                present, node = False, None
                break
        fields.append({"path": path, "present": present, "value": node})
    return fields


def build_name(build):
    return f"tag {build['tag']}" if build.get("tag") else f"commit {build['commit']}"


def entry_from_record(record, fixture, hand, read_log):
    """The entry: hand-written fields from HAND, every other field from the record.

    READ_LOG(name) returns a log's text. check-site.py calls this again on the
    committed logs and compares, so the page cannot drift from the runs.
    """
    tool, version = record["tool"], record["version"]
    any_cut = False

    def shown(run, spec_show, spec_json, where):
        nonlocal any_cut
        text = read_log(run["log"])
        lines = log_lines(text)
        picked = cut(lines, spec_show, where)
        any_cut = any_cut or len(picked) < len(lines)
        out = {"command": run["command"], "output": picked, "exit": run["exit"]}
        fields = json_fields(text, spec_json, where)
        if fields is not None:
            out["fields"] = fields
        return out

    entry = {"_about": ABOUT, "tool": tool, "version": version}
    for key in HAND:
        entry[key] = hand.get(key, "" if key != "lead" else [])
    entry["folder"] = fixture["folder"]
    for side in SIDES:
        data = record["sides"][side]
        runs = [shown(run, spec["show"][side], spec.get("json"), f"{side} run {n + 1} ({run['command']})")
                for n, (run, spec) in enumerate(zip(data["runs"], fixture["runs"]))]
        entry[side] = {
            "build": record["builds"][side]["version_output"],
            "runs": runs,
            "result": {"label": fixture["result"][side]["label"], "files": data["result_files"]},
            "exit": runs[0]["exit"],
        }
    nxt = shown(record["next"], fixture["next"]["show"], fixture["next"].get("json"),
                f"next ({record['next']['command']})")
    entry["next"] = {"note": hand.get("next_note", []), **nxt}
    if hand.get("film"):
        entry["film"] = hand["film"]
    before, after = record["builds"]["before"], record["builds"]["after"]
    entry["builds"] = {
        "before": {"name": build_name(before), "tag": before.get("tag"), "commit": before["commit"],
                   "rev": before["rev"], "sha256": before["sha256"]},
        "after": {"commit": after["commit"], "rev": after["rev"], "sha256": after["sha256"]},
    }
    # The part of the footer that does not depend on the tag. build-site.py
    # puts the two builds in front of it.
    entry["runs_note"] = (
        f"Each ran the same commands on a fresh copy of one {fixture.get('kind', 'synthetic folder')}, "
        f"{fixture['folder']}: {fixture['describe']}. "
        + ("Output is shortened to fit by cutting whole lines. " if any_cut else "")
        + SANDBOX_NOTE)
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


def key_guard(tool, ref, rev):
    """Refuse a build that would ask the login keychain for its journal key."""
    if tool not in KEY_TOOLS:
        return
    found = subprocess.run(["git", "grep", "-q", "-F", "ETUDE_JOURNAL_KEY", rev, "--", "crates/"],
                           cwd=ROOT, capture_output=True)
    if found.returncode == 1:
        raise RunError(f"{tool} at {ref} ({rev[:7]}) does not read ETUDE_JOURNAL_KEY, so it would "
                       f"take its journal key from the login keychain through {SECURITY}. "
                       "Nothing was built or run. Pick a ref from 0.5.3 on.")
    if found.returncode != 0:
        raise RunError(f"cannot search {ref} for ETUDE_JOURNAL_KEY: {found.stderr.decode().strip()}")


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
        # Only the worktree this run made; never a prune of anyone else's.
        git("worktree", "remove", "--force", str(src), check=False)


# --- the sandbox --------------------------------------------------------------

def sandbox_profile(root):
    """Writes only under ROOT (and /dev/null, /dev/tty); /usr/bin/security never runs."""
    real = os.path.realpath(root)
    return f"""(version 1)
(allow default)
(deny file-write*
  (require-not (require-any (subpath "{real}") (literal "/dev/null") (literal "/dev/tty")
                            (literal "/dev/dtracehelper"))))
(deny process-exec* (literal "{SECURITY}"))
"""


def security_shim(folder):
    """A `security` that only refuses, for the front of PATH."""
    folder.mkdir(parents=True, exist_ok=True)
    shim = folder / "security"
    shim.write_text("#!/bin/sh\necho 'security: refused. release-runs.py never lets a run "
                    "reach the keychain.' >&2\nexit 77\n")
    shim.chmod(0o755)
    return folder


def sandboxed(argv, root):
    return ["/usr/bin/sandbox-exec", "-p", sandbox_profile(root), *argv]


def force_rmtree(path):
    """Remove PATH even where a fixture made a folder unreadable. Never follows links."""
    if not os.path.lexists(path):
        return
    for top, dirs, _ in os.walk(path, onerror=lambda e: None):
        for name in dirs:
            child = os.path.join(top, name)
            if not os.path.islink(child):
                try:
                    os.chmod(child, stat.S_IRWXU)
                except OSError:
                    pass
    shutil.rmtree(path, ignore_errors=True)


# --- runs --------------------------------------------------------------------

def listing(folder):
    out = []
    for top, dirs, files in os.walk(folder, onerror=lambda e: None):
        for name in dirs + files:
            path = Path(top) / name
            rel = path.relative_to(folder).as_posix()
            if path.is_symlink():
                out.append(f"{rel} -> {os.readlink(path)}")
            elif path.is_dir():
                out.append(rel + "/" + ("" if os.access(path, os.R_OK | os.X_OK) else " (unreadable)"))
            else:
                out.append(rel)
    return sorted(out)


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
    """One fresh folder: work/ for the fixture, home/, state/, tmp/, bin/."""

    def __init__(self, base, name, key):
        self.root = base / f"run-{name}"
        self.work = self.root / "work"
        for sub in ("work", "home", "state", "tmp"):
            (self.root / sub).mkdir(parents=True)
        shim = security_shim(self.root / "bin")
        self.env = {"PATH": f"{shim}:{PATH}", "HOME": str(self.root / "home"),
                    "ETUDE_STATE_DIR": str(self.root / "state"),
                    "ETUDE_JOURNAL_KEY": key,
                    "TMPDIR": str(self.root / "tmp") + "/", "LANG": "en_US.UTF-8"}

    def normalize(self, data):
        """The output with this folder's path written as <scratch>; nothing else changes."""
        text = data.decode("utf-8")
        for path in sorted({os.path.realpath(self.root), str(self.root)}, key=len, reverse=True):
            text = text.replace(path, SCRATCH)
        return text.encode("utf-8")

    def exec(self, argv, timeout=300):
        try:
            return subprocess.run(sandboxed(argv, self.root), cwd=self.work, env=self.env,
                                  stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                  stderr=subprocess.STDOUT, timeout=timeout)
        except subprocess.TimeoutExpired as error:
            raise RunError(f"{' '.join(argv)} did not finish within {timeout} s; it was stopped") from error

    def setup(self, fixture):
        script = self.root / "setup.sh"
        script.write_text("\n".join(fixture["setup"]) + "\n", encoding="utf-8")
        done = self.exec(["/bin/sh", "-eu", str(script)], timeout=60)
        if done.returncode != 0:
            raise RunError(f"the fixture setup failed (exit {done.returncode}): "
                           f"{done.stdout.decode(errors='replace').strip()}")

    def run(self, tool, binary, command, logs, name):
        argv = shlex.split(command)
        if not argv or argv[0] != tool:
            raise RunError(f"the fixture command {command!r} does not start with {tool}")
        done = self.exec([str(binary), *argv[1:]])
        try:
            data = self.normalize(done.stdout)
        except UnicodeDecodeError as error:
            raise RunError(f"{command!r} printed output that is not UTF-8: {error}") from error
        if MACHINE_PATH.search(data.decode("utf-8")):
            raise RunError(f"{command!r} printed a path outside its scratch folder; "
                           "the log would carry it into the repository")
        (logs / name).write_bytes(data)
        return {"command": command, "exit": done.returncode, "log": name,
                "sha256": hashlib.sha256(data).hexdigest()}


def run_side(tool, side, binary, fixture, base, logs):
    key = secrets.token_hex(32)
    main = Scratch(base, side, key)
    main.setup(fixture)
    before = listing(main.work)
    runs = []
    for n, spec in enumerate(fixture["runs"]):
        place = main
        if spec.get("fresh"):
            place = Scratch(base, f"{side}-{n + 1}", key)
            place.setup(fixture)
        runs.append(place.run(tool, binary, spec["command"], logs, f"{side}-{n + 1}.txt"))
    data = {"listing_before": before, "runs": runs,
            "result_files": result_files(main.work, fixture["result"][side],
                                         {p.split(" -> ")[0].split("/ ")[0].rstrip("/") for p in before})}
    nxt = None
    if side == "after":
        nxt = main.run(tool, binary, fixture["next"]["command"], logs, "next.txt")
    data["listing_after"] = listing(main.work)
    return data, nxt


def version_of(binary, base, side):
    scratch = Scratch(base, f"version-{side}", secrets.token_hex(32))
    done = scratch.exec([str(binary), "--version"], timeout=60)
    if done.returncode != 0:
        raise RunError(f"{binary.name} --version exited {done.returncode}")
    return done.stdout.decode().strip()


def write_entry(entry_path, entry):
    entry_path.write_text(json.dumps(entry, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def cuts_only(tool, version, entry_path, logs):
    try:
        fixture = load_fixture(fixture_path(tool, version))
        record = json.loads((logs / "record.json").read_text(encoding="utf-8"))
        if record.get("runs_sha256") != runs_digest(fixture):
            raise RunError("the fixture's setup, commands or result globs changed since the runs; "
                           "run the builds again with --before and --after")
        for run in [r for side in SIDES for r in record["sides"][side]["runs"]] + [record["next"]]:
            if sha256_file(logs / run["log"]) != run["sha256"]:
                raise RunError(f"{logs / run['log']} differs from its sha256 in record.json")
        old = json.loads(entry_path.read_text(encoding="utf-8")) if entry_path.is_file() else {}
        entry = entry_from_record(record, fixture, hand_fields(old),
                                  lambda n: (logs / n).read_text(encoding="utf-8"))
    except (RunError, OSError, json.JSONDecodeError) as error:
        print(f"FAIL {error}", file=sys.stderr)
        return 1
    write_entry(entry_path, entry)
    print(f"ok   wrote {entry_path.relative_to(ROOT)} from the raw logs in {logs}")
    return 0


def stop_on_term(signum, frame):
    # Turn SIGTERM into an exit, so the finally blocks remove the temp folder
    # and the worktrees. SIGINT already raises KeyboardInterrupt.
    raise SystemExit(128 + signum)


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
    signal.signal(signal.SIGTERM, stop_on_term)

    try:
        # The keychain guard comes first, before anything else is read or made.
        refs = {"before": args.before, "after": args.after}
        revs = {side: resolve(refs[side]) for side in SIDES}
        for side in SIDES:
            key_guard(args.tool, refs[side], revs[side])
        fixture = load_fixture(fixture_path(args.tool, args.version))
        old = json.loads(entry_path.read_text(encoding="utf-8")) if entry_path.is_file() else {}
        if not os.access("/usr/bin/sandbox-exec", os.X_OK):
            raise RunError("/usr/bin/sandbox-exec is missing; the runs need it. Nothing was built or run.")
        tmp = Path(tempfile.mkdtemp(prefix="etudes-release-runs-"))
        try:
            target = Path(args.target_dir).resolve() if args.target_dir else tmp / "target"
            staged = tmp / "logs"
            staged.mkdir()
            record = {"tool": args.tool, "version": args.version,
                      "fixture": fixture_path(args.tool, args.version).name,
                      "runs_sha256": runs_digest(fixture),
                      "env": [f"PATH={SCRATCH}/bin:{PATH} (bin holds a security that only refuses)",
                              f"HOME={SCRATCH}/home", f"ETUDE_STATE_DIR={SCRATCH}/state",
                              "ETUDE_JOURNAL_KEY=<64 random hex, not recorded>",
                              f"TMPDIR={SCRATCH}/tmp/", "LANG=en_US.UTF-8", "stdin closed",
                              "sandbox-exec: no file writes outside the scratch folder, "
                              f"no exec of {SECURITY}"],
                      "logs": f"each log is the command's output with the scratch folder's path "
                              f"written as {SCRATCH}",
                      "builds": {}, "sides": {}}
            for side in SIDES:
                binary = build(args.tool, refs[side], revs[side], tmp, target, side)
                version_line = version_of(binary, tmp, side)
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
            entry = entry_from_record(record, fixture, hand_fields(old),
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
            write_entry(entry_path, entry)
        finally:
            force_rmtree(tmp)
    except RunError as error:
        print(f"FAIL {error}", file=sys.stderr)
        return 1
    missing = [k for k in HAND if not entry.get(k)]
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
