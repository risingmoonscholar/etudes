#!/usr/bin/env python3
"""Fail when a test can run a tool that may reach the login keychain.

A tool run with no journal key asks the login keychain for one, and may add
one. Tests never do that. Every test that runs a workspace binary goes through
the shared helper in crates/etude-cli-support/test-support/test_tool.rs:

  keyed_tool(path)        a command for the tool, with a test key, state
                          folder and HOME
  with_test_key(&mut cmd) the same settings for a wrapper (a shell, a script)
                          that runs the tool
  fenced_tool(path)       a run with no key, where the keychain program
                          cannot start

The rules are per spawn: each statement that starts a tool must itself be keyed.
For every Rust file under crates/*/tests, crates/*/src and crates/*/test-support:

  1. A binary path (env!("CARGO_BIN_EXE_..."), a constant holding one, or the
     name of a workspace binary, from [[bin]] or src/bin/*.rs or a package's
     src/main.rs) given straight to Command::new fails; use keyed_tool.
  2. Any other mention of a binary path must be in a keyed statement: one that
     calls with_test_key, keyed_tool or fenced_tool, or that continues a
     command already passed to one of them. A `let` that only holds the path
     passes the rule on to every later statement that uses the name.
  3. `cargo run`, a Command::new of a path built from current_exe(), and a
     string naming target/debug/ or target/release/ are spawns of a workspace
     binary too. A current_exe() spawn (a unit test running its own test
     binary, where the helper is not available) may instead set
     ETUDE_JOURNAL_KEY itself in the same statement.
  4. env_remove("ETUDE_JOURNAL_KEY") and env_clear() fail outside a function
     marked fenced.
  5. A function marked with a `// keychain-fenced:` comment directly above it
     is accepted, but it must actually fence: call fenced_tool, or spawn
     sandbox-exec.
  6. A binary path outside any function is only allowed as a constant; its
     uses are checked by the rules above.

Matches inside comments and string literals never count as a helper call or a
fence. Usage: scripts/check-test-keys.py [--root DIR]
Exit:  0 every spawn is keyed or fenced, 1 at least one is not.
"""

import argparse
import pathlib
import re
import sys

HELPER = "crates/etude-cli-support/test-support/test_tool.rs"
HELPER_CALL = re.compile(r"\b(?:keyed_tool|with_test_key|fenced_tool)\s*\(")
FENCE_SPAWN = re.compile(r'\bfenced_tool\s*\(|Command::new\(\s*"(?:/usr/bin/)?sandbox-exec"')
MARKER = "keychain-fenced:"
KEYWORDS = {"let", "mut", "ref", "Some", "Ok", "Err", "None", "else", "if", "match", "_"}


def code_mask(text):
    """Return (code, in_string): code has comments blanked; in_string marks
    the characters inside a string literal (not the quotes)."""
    out = list(text)
    in_string = [False] * len(text)
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if text.startswith("//", i):
            j = text.find("\n", i)
            j = n if j < 0 else j
            for k in range(i, j):
                out[k] = " "
            i = j
        elif text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif text.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            for k in range(i, j):
                if out[k] != "\n":
                    out[k] = " "
            i = j
        elif re.match(r'b?r(#*)"', text[i:i + 40]) and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            m = re.match(r'b?r(#*)"', text[i:])
            end = text.find('"' + m.group(1), i + m.end())
            end = n if end < 0 else end + 1 + len(m.group(1))
            for k in range(i + m.end(), end - 1 - len(m.group(1))):
                in_string[k] = True
            i = end
        elif c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            for k in range(i + 1, min(j, n)):
                in_string[k] = True
            i = j + 1
        elif c == "'" and re.match(r"'(\\.|[^\\'])'", text[i:i + 4]):
            i += len(re.match(r"'(\\.|[^\\'])'", text[i:]).group(0))
        else:
            i += 1
    return "".join(out), in_string


def blank_strings(code, in_string):
    return "".join(" " if s else c for c, s in zip(code, in_string))


def functions(bare):
    """Yield (fn_start, body_start, body_end) for every fn, from string-free code."""
    for m in re.finditer(r"\bfn\s+\w+", bare):
        depth, j = 0, m.end()
        while j < len(bare):
            ch = bare[j]
            if ch in "(<[":
                depth += 1
            elif ch in ")>]":
                depth -= 1
            elif ch == ";" and depth <= 0:
                break
            elif ch == "{" and depth <= 0:
                break
            j += 1
        if j >= len(bare) or bare[j] != "{":
            continue
        start, depth = j, 0
        while j < len(bare):
            if bare[j] == "{":
                depth += 1
            elif bare[j] == "}":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        yield m.start(), start, j


def marked(text, fn_start):
    """True if the comment and attribute lines directly above a fn carry the marker."""
    lines = text[:fn_start].split("\n")
    lines.pop()
    while lines:
        line = lines.pop().strip()
        if line.startswith("//") or line.startswith("#["):
            if line.startswith("//") and MARKER in line:
                return True
            continue
        return False
    return False


def statement(bare, pos, lo, hi):
    """The statement around pos, within [lo, hi): bounded by ; { } at bracket depth 0."""
    depth, i = 0, pos - 1
    while i > lo:
        ch = bare[i]
        if ch in ")]":
            depth += 1
        elif ch in "([":
            depth = max(depth - 1, 0) if depth else 0
        elif ch in ";{}" and depth == 0:
            break
        i -= 1
    start = i + 1
    depth, j = 0, pos
    while j < hi:
        ch = bare[j]
        if ch in "([":
            depth += 1
        elif ch in ")]":
            depth -= 1
            if depth < 0:
                depth = 0
        elif ch in ";{}" and depth == 0:
            break
        j += 1
    return start, j


def binary_names(root):
    names = set()
    for manifest in root.glob("crates/*/Cargo.toml"):
        text = manifest.read_text()
        for block in re.findall(r"\[\[bin\]\]([^\[]*)", text):
            m = re.search(r'name\s*=\s*"([^"]+)"', block)
            if m:
                names.add(m.group(1))
        crate = manifest.parent
        # Cargo's automatic binaries.
        names.update(p.stem for p in crate.glob("src/bin/*.rs"))
        names.update(p.name for p in crate.glob("src/bin/*/") if (p / "main.rs").exists())
        package = re.search(r'\[package\][^\[]*?name\s*=\s*"([^"]+)"', text)
        if package and (crate / "src/main.rs").exists() and "[[bin]]" not in text:
            names.add(package.group(1))
    return names


def sources(root):
    files = []
    for pattern in ("crates/*/tests/**/*.rs", "crates/*/src/**/*.rs", "crates/*/test-support/*.rs"):
        files.extend(root.glob(pattern))
    helper = (root / HELPER).resolve()
    return sorted({f for f in files if f.resolve() != helper})


def line_of(text, pos):
    return text.count("\n", 0, pos) + 1


class Function:
    def __init__(self, bare, start, body_start, body_end):
        self.bare, self.start, self.lo, self.hi = bare, start, body_start + 1, body_end
        self.keyed_vars = set()
        body = bare[self.lo:self.hi]
        for m in re.finditer(r"\bwith_test_key\s*\(\s*&mut\s+(\w+)\s*\)", body):
            self.keyed_vars.add(m.group(1))
        for m in re.finditer(r"\blet\s+(?:mut\s+)?(\w+)\s*(?::[^=]*)?=\s*(?:keyed_tool|fenced_tool|with_test_key)\s*\(", body):
            self.keyed_vars.add(m.group(1))

    def keyed(self, start, end):
        text = self.bare[start:end]
        if HELPER_CALL.search(text):
            return True
        lead = re.match(r"\s*(?:let\s+[^=]*=\s*)?(\w+)\s*\.", text)
        return bool(lead and lead.group(1) in self.keyed_vars)

    def bound_names(self, start, end):
        m = re.match(r"\s*let\s+([^=]*?)\s*(?::[^=]*)?=", self.bare[start:end])
        if not m:
            return set()
        return {w for w in re.findall(r"[A-Za-z_]\w*", m.group(1)) if w not in KEYWORDS}

    def follow(self, pos, depth=0):
        """None if the use at pos is in a keyed statement (or only bound for
        later keyed use); otherwise the position of an unkeyed use."""
        start, end = statement(self.bare, pos, self.lo, self.hi)
        if self.keyed(start, end):
            return None
        names = self.bound_names(start, end)
        if not names or depth > 3:
            return pos
        for name in names:
            for m in re.finditer(r"\b%s\b" % re.escape(name), self.bare[end:self.hi]):
                bad = self.follow(end + m.start(), depth + 1)
                if bad is not None:
                    return bad
        return None


def check(root):
    root = pathlib.Path(root)
    files = {path: path.read_text() for path in sources(root)}
    lexed = {}
    for path, text in files.items():
        code, in_string = code_mask(text)
        lexed[path] = (code, in_string, blank_strings(code, in_string))

    constants = set()
    for code, _, _ in lexed.values():
        constants.update(re.findall(
            r"\b(?:const|static)\s+([A-Z_][A-Z0-9_]*)\s*:[^=;]*=\s*env!\(\s*\"CARGO_BIN_EXE_", code))
    names = binary_names(root)

    problems = []
    for path, text in files.items():
        code, in_string, bare = lexed[path]
        rel = path.relative_to(root)
        fns = [Function(bare, *span) for span in functions(bare)]

        def enclosing(pos):
            inside = [f for f in fns if f.lo <= pos < f.hi]
            return min(inside, key=lambda f: f.hi - f.lo) if inside else None

        # (position, how it is written, kind)
        hits = []
        for m in re.finditer(r'env!\(\s*"CARGO_BIN_EXE_[^"]*"\s*\)', code):
            hits.append((m.start(), m.group(0), "path"))
        for name in constants:
            for m in re.finditer(r"\b%s\b" % name, bare):
                hits.append((m.start(), name, "path"))
        for m in re.finditer(r'Command::new\(\s*"([^"]+)"', code):
            program = m.group(1)
            if program in names or any(program.endswith("/" + b) for b in names):
                hits.append((m.start() + len("Command::new("), f'"{program}"', "path"))
            elif program == "cargo":
                fn = enclosing(m.start())
                if fn:
                    s, e = statement(bare, m.start(), fn.lo, fn.hi)
                    if re.search(r'"run"', code[s:e]):
                        hits.append((m.start(), "cargo run", "spawn"))
        for m in re.finditer(r"target/(?:debug|release)/", code):
            if in_string[m.start()]:
                hits.append((m.start(), m.group(0), "spawn"))
        for m in re.finditer(r"\bcurrent_exe\s*\(", bare):
            hits.append((m.start(), "current_exe()", "exe"))

        for pos, written, kind in sorted(hits):
            line = line_of(text, pos)
            fn = enclosing(pos)
            if fn is None:
                before = code[:pos].rstrip()
                if kind == "path" and (re.search(r"\b(?:const|static)\s+[A-Z_][A-Z0-9_]*\s*:[^=;]*=\s*$", before)
                                       or re.match(r"\s*:", code[pos + len(written):])):
                    continue
                if kind == "exe":
                    continue
                problems.append(f"{rel}:{line}: {written} outside a function; hold it in a const or move it into a keyed function")
                continue
            if marked(text, fn.start):
                continue  # judged once per function below
            if kind == "exe":
                problem = exe_problem(fn, code, bare, pos, rel, line)
                if problem:
                    problems.append(problem)
                continue
            before = bare[:pos].rstrip()
            if kind == "path" and before.endswith("Command::new("):
                problems.append(f"{rel}:{line}: Command::new({written}) runs a tool with no test key; use keyed_tool({written})")
                continue
            if kind == "path" and re.search(r"\b(?:keyed_tool|fenced_tool)\s*\($", before):
                continue
            bad = fn.follow(pos)
            if bad is not None:
                where = line_of(text, bad)
                problems.append(f"{rel}:{where}: {written} reaches a statement that never calls "
                                "with_test_key, keyed_tool or fenced_tool")

        for m in re.finditer(r'\benv_remove\(\s*"ETUDE_JOURNAL_KEY"\s*\)|\benv_clear\(\s*\)', code):
            if in_string[m.start()]:
                continue
            fn = enclosing(m.start())
            if fn is None or not marked(text, fn.start):
                problems.append(f"{rel}:{line_of(text, m.start())}: {m.group(0)} outside a function marked "
                                f"{MARKER}; a run with no key must be fenced")

        for fn in fns:
            if marked(text, fn.start) and not FENCE_SPAWN.search(blank_quoted_except_command(code, in_string, fn)):
                problems.append(f"{rel}:{line_of(text, fn.start)}: marked {MARKER} but never fences "
                                "(no fenced_tool or sandbox-exec spawn)")

    helper = root / HELPER
    if helper.exists():
        helper_text = helper.read_text()
        for needed in ('"ETUDE_JOURNAL_KEY"', '"ETUDE_STATE_DIR"', '"HOME"', 'deny process-exec (literal "/usr/bin/security")'):
            if needed not in helper_text:
                problems.append(f"{HELPER}: no longer mentions {needed}")
    elif files:
        problems.append(f"{HELPER}: missing")
    return problems


def blank_quoted_except_command(code, in_string, fn):
    """The function body with string contents blanked, except a string that is
    the program given to Command::new (so a sandbox-exec spawn still counts,
    and a mere mention of sandbox-exec in a message does not)."""
    body = list(code[fn.lo:fn.hi])
    keep = set()
    for m in re.finditer(r'Command::new\(\s*"([^"]*)"', code[fn.lo:fn.hi]):
        keep.update(range(m.start(1), m.end(1)))
    for k in range(len(body)):
        if in_string[fn.lo + k] and k not in keep:
            body[k] = " "
    return "".join(body)


def exe_problem(fn, code, bare, pos, rel, line):
    """A path built from current_exe() is a spawn when it reaches Command::new."""
    start, end = statement(bare, pos, fn.lo, fn.hi)
    stmt = bare[start:end]
    spawned = re.search(r"Command::new\([^;]*current_exe\s*\(", stmt)
    names = fn.bound_names(start, end)
    uses = []
    if not spawned:
        for name in names:
            for m in re.finditer(r"Command::new\(\s*&?%s\b" % re.escape(name), bare[end:fn.hi]):
                uses.append(end + m.start())
        if not uses:
            return None
    for at in ([pos] if spawned else uses):
        s, e = statement(bare, at, fn.lo, fn.hi)
        if fn.keyed(s, e) or re.search(r'\.env\(\s*"ETUDE_JOURNAL_KEY"', code[s:e]):
            continue
        return (f"{rel}:{line}: a binary built from current_exe() is started with no journal key; "
                "use keyed_tool, or set ETUDE_JOURNAL_KEY in the same statement")
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", default=pathlib.Path(__file__).resolve().parent.parent)
    args = parser.parse_args()
    problems = check(args.root)
    for problem in problems:
        print(f"FAIL {problem}")
    if problems:
        print(f"{len(problems)} test spawn(s) can reach the login keychain. "
              "See crates/etude-cli-support/test-support/test_tool.rs.")
        return 1
    print("ok: every test that runs a tool gives it a key or fences it")
    return 0


if __name__ == "__main__":
    sys.exit(main())
