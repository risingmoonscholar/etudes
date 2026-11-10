"""A planted bug in front of a real études binary.

stress/mustfire/run.sh puts a shim named sweep, stash or unpack in a scratch
BIN folder. The shim runs this file with the tool name and the original
arguments. MUSTFIRE_REAL_BIN names the folder with the real binaries, and
MUSTFIRE_MUTANT names one planted bug from MUTANTS below.

A call that the planted bug does not target runs the real binary unchanged
(exec, same process id), so a scenario that signals or watches that process
sees the real tool. A targeted call runs the real binary, then does one wrong
thing that a real defect could do: lose a file, change bytes, hide a line,
swallow a signal, or report the wrong exit or reason code.

Each planted bug must be found by its scenario: the scenario must report
FAIL. A pass or an unproven result means the scenario cannot see that bug.

This file never runs /usr/bin/security and never reads files outside the
folders that the tool call names (or the scenario's own scratch folder).
"""
import json
import os
import plistlib
import re
import signal
import subprocess
import sys
from pathlib import Path

VALUE_FLAGS = {
    "--depth", "--since", "--map", "--scheme", "--export-plan", "--plan",
    "--plan-digest", "--proposal", "--id", "--item", "--into", "--only",
}
SUBCOMMANDS = {
    "sweep": {"review", "apply", "propose", "undo", "status", "forget", "verify",
              "contract", "lesson"},
    "stash": {"pop", "status", "inspect", "select", "list", "forget", "verify",
              "contract"},
    "unpack": set(),
}
TAG_XATTR = "com.apple.metadata:_kMDItemUserTags"


def real(tool):
    return str(Path(os.environ["MUSTFIRE_REAL_BIN"]) / tool)


def split(tool, args):
    sub = args[0] if args and args[0] in SUBCOMMANDS[tool] else ""
    rest = args[1:] if sub else args
    paths, skip = [], False
    for arg in rest:
        if skip:
            skip = False
            continue
        if arg in VALUE_FLAGS:
            skip = True
            continue
        if not arg.startswith("-") and os.path.lexists(arg):
            paths.append(Path(arg))
    return sub, paths


def roots(paths):
    """The folders a call works in: each folder argument, or a file's parent."""
    found = []
    for p in paths:
        r = p if p.is_dir() else p.parent
        if r not in found:
            found.append(r)
    if not found:
        # `sweep undo` with no folder: use the scenario's own scratch folder.
        scen = os.environ.get("SCENARIO", "")
        tmp = Path(os.environ.get("TMPDIR", "/tmp"))
        mine = sorted(tmp.glob(f"etudes-stress-{scen}-*"), key=lambda d: d.stat().st_mtime)
        found = mine[-1:]
    return found


def walk(rs, hidden=False):
    """Every regular file under the roots, as a set of absolute paths."""
    out = set()
    for r in rs:
        for dirpath, dirs, files in os.walk(r):
            if not hidden:
                dirs[:] = [d for d in dirs if not d.startswith(".")]
            for f in files:
                p = Path(dirpath) / f
                if p.is_file() and not p.is_symlink() and (hidden or not f.startswith(".")):
                    out.add(p)
    return out


def run(tool, args, capture=True):
    return subprocess.run([real(tool), *args], capture_output=capture)


def emit(proc, out=None, err=None):
    sys.stdout.buffer.write(proc.stdout if out is None else out)
    sys.stderr.buffer.write(proc.stderr if err is None else err)
    sys.stdout.flush()
    sys.stderr.flush()


def first(paths, prefer=None):
    paths = sorted(paths, key=str)
    if prefer:
        chosen = [p for p in paths if prefer(p)]
        if chosen:
            return chosen[0]
    return paths[0] if paths else None


def change_bytes(p):
    data = p.read_bytes()
    p.write_bytes(data[: len(data) // 2] if data else b"x")


def json_edit(raw, edit):
    """Apply edit(obj) to each JSON object line in raw; keep other lines."""
    out = []
    for line in raw.decode(errors="surrogateescape").splitlines(keepends=True):
        try:
            obj = json.loads(line)
        except ValueError:
            try:
                obj = json.loads(raw)
            except ValueError:
                out.append(line)
                continue
            edit(obj)
            return (json.dumps(obj) + "\n").encode()
        edit(obj)
        out.append(json.dumps(obj) + "\n")
    return "".join(out).encode(errors="surrogateescape")


def drop_lines(raw, pattern):
    keep = [l for l in raw.splitlines(keepends=True) if not re.search(pattern, l)]
    return b"".join(keep)


def set_tags(p, names):
    blob = plistlib.dumps(names, fmt=plistlib.FMT_BINARY)
    subprocess.run(["chflags", "nouchg", str(p)], capture_output=True)
    subprocess.run(["xattr", "-wx", TAG_XATTR, blob.hex(), str(p)], capture_output=True)
    subprocess.run(["chflags", "uchg", str(p)], capture_output=True)


def is_locked(p):
    return bool(os.lstat(p).st_flags & 0x2)  # UF_IMMUTABLE


# --- the planted bugs --------------------------------------------------------
# Each entry: (tool, subcommand it targets, function). The subcommand "" is the
# bare command (sweep plan, stash put, unpack). "*" targets every call.

def apply_noop(tool, args, rs):
    """Apply prints its normal result and exits 0, but every file is back where it was."""
    p = run(tool, args)
    if p.returncode == 0:
        subprocess.run([real(tool), "undo", *map(str, rs)], capture_output=True)
    emit(p)
    return p.returncode


def apply_returns_odd_name(tool, args, rs):
    """Apply moves everything except one file with an unusual name, and says nothing."""
    return _apply_returns(tool, args, rs, lambda f: re.search(r"[^A-Za-z0-9._ -]", f.name))


def apply_returns_longest_name(tool, args, rs):
    """Apply moves everything except the file with the longest path, and says nothing."""
    return _apply_returns(tool, args, rs, None, longest=True)


def _apply_returns(tool, args, rs, prefer, longest=False):
    before = walk(rs)
    p = run(tool, args)
    moved = before - walk(rs)
    if longest:
        odd = max(moved, key=lambda f: (len(str(f)), str(f)), default=None)
    else:
        odd = first(moved, prefer=prefer)
    if p.returncode == 0 and odd is not None:
        after = walk(rs) - before
        landed = first(after, prefer=lambda f: f.name == odd.name)
        if landed is not None and not odd.exists():
            os.rename(landed, odd)
    emit(p)
    return p.returncode


def apply_returns_empty_files(tool, args, rs):
    """Apply moves the files with bytes, but leaves every empty file where it was, and says nothing."""
    before = {f: f.stat().st_size for f in walk(rs)}
    p = run(tool, args)
    if p.returncode == 0:
        landed = walk(rs) - set(before)
        for src in sorted((f for f, size in before.items() if size == 0 and not f.exists()), key=str):
            match = first(landed, prefer=lambda f, n=src.name: f.name == n)
            if match is not None and match.name == src.name:
                os.rename(match, src)
                landed.discard(match)
    emit(p)
    return p.returncode


def apply_swaps_case_pair(tool, args, rs):
    """Apply lands two names that differ only in case, but with each other's bytes."""
    p = run(tool, args)
    if p.returncode == 0:
        by_dir = {}
        for f in walk(rs):
            by_dir.setdefault((f.parent, f.name.lower()), []).append(f)
        pairs = [v for v in by_dir.values() if len(v) == 2]
        if pairs:
            a, b = sorted(pairs[0], key=str)
            da, db = a.read_bytes(), b.read_bytes()
            a.write_bytes(db)
            b.write_bytes(da)
    emit(p)
    return p.returncode


def apply_moves_held_audio(tool, args, rs):
    """Apply prints the hold line, then moves the held audio anyway."""
    p = run(tool, args)
    if p.returncode == 0:
        for r in rs:
            loose = [f for f in r.iterdir() if f.is_file()
                     and f.suffix.lower() in {".wav", ".aif", ".aiff", ".mp3", ".flac"}]
            if loose:
                (r / "Audio").mkdir(exist_ok=True)
                for f in loose:
                    os.rename(f, r / "Audio" / f.name)
    emit(p)
    return p.returncode


def apply_drops_journal(tool, args, rs):
    """Apply moves the files and exits 0, but its journal is gone."""
    p = run(tool, args)
    if p.returncode == 0:
        state = Path(os.environ["ETUDE_STATE_DIR"])
        for f in walk([state], hidden=True):
            f.unlink()
    emit(p)
    return p.returncode


def tags_locked_file(tool, args, rs):
    """The tag run also tags a locked file."""
    p = run(tool, args)
    if p.returncode == 0:
        locked = first([f for f in walk(rs) if is_locked(f)])
        if locked is not None:
            set_tags(locked, ["Red\n6"])
    emit(p)
    return p.returncode


def undo_changes_bytes(tool, args, rs):
    """Undo puts every file back, but one restored file has different bytes."""
    before = walk(rs)
    p = run(tool, args)
    back = walk(rs) - before
    if p.returncode == 0 and back:
        change_bytes(first(back))
    emit(p)
    return p.returncode


def undo_changes_mtime(tool, args, rs):
    """Undo puts every file back, but one restored file has a new modification time."""
    before = walk(rs)
    p = run(tool, args)
    back = walk(rs) - before
    if p.returncode == 0 and back:
        os.utime(first(back))
    emit(p)
    return p.returncode


def undo_loses_file(tool, args, rs):
    """Undo restores all files but one, which is gone from every folder."""
    before = walk(rs)
    p = run(tool, args)
    back = walk(rs) - before
    if back:
        first(back).unlink()
    emit(p)
    return p.returncode


def undo_exits_1(tool, args, rs):
    """Undo restores every file, then exits 1."""
    p = run(tool, args)
    emit(p)
    return 1 if p.returncode == 0 else p.returncode


def undo_short_journal_is_nothing(tool, args, rs):
    """A journal cut short is treated as no journal: "nothing to undo", exit 1, no damage named."""
    state = Path(os.environ["ETUDE_STATE_DIR"])
    journals = list(state.glob("sweep-*.journal"))
    if journals and all(j.stat().st_size <= 50 for j in journals):
        sys.stderr.write("sweep: nothing to undo\n")
        return 1
    p = run(tool, args)
    emit(p)
    return p.returncode


def plan_hides_unreadable(tool, args, rs):
    """The plan says nothing about a folder it could not read."""
    p = run(tool, args)
    hide = lambda b: re.sub(rb'("unreadable"\s*:\s*)[0-9]+', rb"\g<1>0",
                            drop_lines(b, rb"(?i)could not be read"))
    emit(p, out=hide(p.stdout), err=hide(p.stderr))
    return p.returncode


def plan_crashes_on_fifo(tool, args, rs):
    """The scan stops with an error when the folder holds a FIFO."""
    import stat
    for r in rs:
        for f in r.iterdir():
            if stat.S_ISFIFO(os.lstat(f).st_mode):
                sys.stderr.write("sweep: scan failed: unexpected file type\n")
                return 1
    p = run(tool, args)
    emit(p)
    return p.returncode


def panics_on_refusal(tool, args, rs):
    """A refusal turns into a crash."""
    p = run(tool, args)
    if 0 < p.returncode < 100:  # a refusal; a signal death stays as it was
        emit(p, err=p.stderr + b"\nthread 'main' panicked at src/main.rs:1:1:\nplanted bug\n")
        return 101
    emit(p)
    return p.returncode


def wrong_reason_code(tool, args, rs):
    """A plan_stale refusal keeps its exit and words but carries the wrong reason code."""
    p = run(tool, args)
    fix = lambda b: b.replace(b"plan_stale", b"planted_wrong_code")
    emit(p, out=fix(p.stdout), err=fix(p.stderr))
    return p.returncode


def plan_never_replays(tool, args, rs):
    """Applying a saved plan always refuses, even when nothing changed."""
    if "--plan" in args:
        sys.stderr.write("sweep: this plan no longer matches the folder (plan_stale)\n")
        if "--json" in args:
            print(json.dumps({"status": "refused", "reason": {"code": "plan_stale"}}))
        return 2
    p = run(tool, args)
    emit(p)
    return p.returncode


def hides_could_not_be_updated(tool, args, rs):
    """Undo no longer says which folder could not be updated."""
    p = run(tool, args)
    pat = rb"(?i)could not be updated"
    emit(p, out=drop_lines(p.stdout, pat), err=drop_lines(p.stderr, pat))
    return p.returncode


def mangles_argument_name(tool, args, rs):
    """The message names a different argument than the one with bad bytes."""
    p = run(tool, args)
    fix = lambda b: re.sub(rb"argument (.+?) has bytes", rb"argument other_name has bytes", b)
    emit(p, out=fix(p.stdout), err=fix(p.stderr))
    return p.returncode


def hides_archive_name(tool, args, rs):
    """The permission message no longer names the archive."""
    p = run(tool, args)
    fix = lambda b: b.replace(b"locked.zip", b"the archive")
    emit(p, out=fix(p.stdout), err=fix(p.stderr))
    return p.returncode


def unpack_does_nothing(tool, args, rs):
    """Unpack exits 0 and extracts nothing."""
    return 0


def unpack_changes_deepest_file(tool, args, rs):
    """Unpack exits 0, but the most deeply nested member has different bytes."""
    p = run(tool, args)
    if p.returncode == 0 and "--into" in args:
        out = Path(args[args.index("--into") + 1])
        files = [f for f in walk([out]) if f.is_file()]
        if files:
            change_bytes(max(files, key=lambda f: (len(f.relative_to(out).parts), str(f))))
    emit(p)
    return p.returncode


def unpack_ignores_term(tool, args, rs):
    """Unpack ignores SIGTERM, finishes, and exits 0."""
    got = []
    signal.signal(signal.SIGTERM, lambda *_: got.append(1))
    p = subprocess.run([real(tool), *args], capture_output=True,
                       preexec_fn=lambda: signal.signal(signal.SIGTERM, signal.SIG_IGN))
    emit(p)
    return 0 if got else p.returncode


def stash_loses_item(tool, args, rs):
    """Stash reports success, but one held file is gone."""
    before = walk(rs, hidden=True)
    p = run(tool, args)
    held = {f for f in walk(rs, hidden=True) - before
            if any(part.startswith(".") for part in f.relative_to(f.anchor).parts)}
    if p.returncode == 0 and held:
        first(held).unlink()
    emit(p)
    return p.returncode


def pop_changes_bytes(tool, args, rs):
    """Pop restores every file, but one has different bytes."""
    return undo_changes_bytes(tool, args, rs)


def pop_loses_file(tool, args, rs):
    """Pop restores all files but one."""
    return undo_loses_file(tool, args, rs)


def pop_exits_1(tool, args, rs):
    """Pop restores every file, then exits 1."""
    return undo_exits_1(tool, args, rs)


def pop_strips_folder_tags(tool, args, rs):
    """Pop restores the files but removes the Finder tags from their folders."""
    p = run(tool, args)
    for r in rs:
        for dirpath, dirs, _ in os.walk(r):
            for d in [Path(dirpath), *(Path(dirpath) / x for x in dirs)]:
                subprocess.run(["xattr", "-d", TAG_XATTR, str(d)], capture_output=True)
    emit(p)
    return p.returncode


def status_drops_id(tool, args, rs):
    """stash status --json no longer carries details.id."""
    p = run(tool, args)
    out = p.stdout
    if "--json" in args:
        out = json_edit(p.stdout, lambda o: o.get("details", {}).pop("id", None))
    emit(p, out=out)
    return p.returncode


MUTANTS = {
    "apply_noop": ("sweep", "apply", apply_noop),
    "apply_returns_odd_name": ("sweep", "apply", apply_returns_odd_name),
    "apply_returns_longest_name": ("sweep", "apply", apply_returns_longest_name),
    "apply_swaps_case_pair": ("sweep", "apply", apply_swaps_case_pair),
    "apply_returns_empty_files": ("sweep", "apply", apply_returns_empty_files),
    "apply_moves_held_audio": ("sweep", "apply", apply_moves_held_audio),
    "apply_drops_journal": ("sweep", "apply", apply_drops_journal),
    "tags_locked_file": ("sweep", "apply", tags_locked_file),
    "undo_changes_bytes": ("sweep", "undo", undo_changes_bytes),
    "undo_changes_mtime": ("sweep", "undo", undo_changes_mtime),
    "undo_loses_file": ("sweep", "undo", undo_loses_file),
    "undo_exits_1": ("sweep", "undo", undo_exits_1),
    "undo_short_journal_is_nothing": ("sweep", "undo", undo_short_journal_is_nothing),
    "plan_hides_unreadable": ("sweep", "", plan_hides_unreadable),
    "plan_crashes_on_fifo": ("sweep", "", plan_crashes_on_fifo),
    "sweep_panics_on_refusal": ("sweep", "*", panics_on_refusal),
    "sweep_wrong_reason_code": ("sweep", "apply", wrong_reason_code),
    "plan_never_replays": ("sweep", "apply", plan_never_replays),
    "hides_could_not_be_updated": ("sweep", "undo", hides_could_not_be_updated),
    "sweep_mangles_argument_name": ("sweep", "*", mangles_argument_name),
    "unpack_hides_archive_name": ("unpack", "*", hides_archive_name),
    "unpack_does_nothing": ("unpack", "*", unpack_does_nothing),
    "unpack_ignores_term": ("unpack", "*", unpack_ignores_term),
    "unpack_changes_deepest_file": ("unpack", "*", unpack_changes_deepest_file),
    "stash_loses_item": ("stash", "", stash_loses_item),
    "stash_select_loses_item": ("stash", "select", stash_loses_item),
    "pop_changes_bytes": ("stash", "pop", pop_changes_bytes),
    "pop_loses_file": ("stash", "pop", pop_loses_file),
    "pop_exits_1": ("stash", "pop", pop_exits_1),
    "pop_strips_folder_tags": ("stash", "pop", pop_strips_folder_tags),
    "status_drops_id": ("stash", "status", status_drops_id),
}


def main():
    tool, args = sys.argv[1], sys.argv[2:]
    mutant = os.environ.get("MUSTFIRE_MUTANT", "")
    if mutant not in MUTANTS:
        sys.stderr.write(f"mustfire: unknown planted bug {mutant!r}\n")
        return 70
    target = MUTANTS[mutant]
    sub, paths = split(tool, args)
    if target is None or target[0] != tool or target[1] not in ("*", sub):
        os.execv(real(tool), [real(tool), *args])
    return target[2](tool, args, roots(paths))


if __name__ == "__main__":
    sys.exit(main())
