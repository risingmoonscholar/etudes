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

The rules, for every Rust file under crates/*/tests, crates/*/src and
crates/*/test-support:

  1. A binary path (env!("CARGO_BIN_EXE_..."), a constant holding one, or the
     bare name of a workspace binary in Command::new) given straight to
     Command::new fails. It must go to keyed_tool or fenced_tool instead.
  2. Any other mention of a binary path must sit in a function that also calls
     with_test_key, keyed_tool or fenced_tool, so the wrapper that runs it gets
     the key.
  3. A function marked with a `// keychain-fenced:` comment above it is
     accepted as it is, but it must actually fence: it has to call fenced_tool
     or sandbox-exec.
  4. A binary path outside any function is only allowed as a constant; its
     uses are then checked by the rules above.

Usage: scripts/check-test-keys.py [--root DIR]
Exit:  0 every spawn is keyed or fenced, 1 at least one is not.
"""

import argparse
import pathlib
import re
import sys

HELPER = "crates/etude-cli-support/test-support/test_tool.rs"
HELPER_CALLS = ("keyed_tool(", "with_test_key(", "fenced_tool(")
FENCE_CALLS = ("fenced_tool(", "sandbox-exec")
MARKER = "keychain-fenced:"


def code_mask(text):
    """Return (code, strings): code has comments blanked; strings marks
    which characters are inside a string or char literal."""
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


def functions(code, in_string):
    """Yield (start_of_signature, body_start, body_end) for every fn."""
    for m in re.finditer(r"\bfn\s+\w+", code):
        if in_string[m.start()]:
            continue
        depth, j = 0, m.end()
        # Find the opening brace of the body, skipping generics and arguments.
        while j < len(code):
            ch = code[j]
            if in_string[j]:
                j += 1
                continue
            if ch in "(<[":
                depth += 1
            elif ch in ")>]":
                depth -= 1
            elif ch == ";" and depth <= 0:
                break  # a declaration with no body
            elif ch == "{" and depth <= 0:
                break
            j += 1
        if j >= len(code) or code[j] != "{":
            continue
        start, depth = j, 0
        while j < len(code):
            if not in_string[j]:
                if code[j] == "{":
                    depth += 1
                elif code[j] == "}":
                    depth -= 1
                    if depth == 0:
                        break
            j += 1
        yield m.start(), start, j


def marked(text, fn_start):
    """True if the comment and attribute lines directly above a fn carry the
    keychain-fenced marker."""
    lines = text[:fn_start].split("\n")
    lines.pop()  # the partial line that holds `fn` (or `pub fn`, `async fn`)
    while lines:
        line = lines.pop().strip()
        if line.startswith("//") or line.startswith("#["):
            if MARKER in line:
                return True
            continue
        return False
    return False


def binary_names(root):
    names = set()
    for manifest in root.glob("crates/*/Cargo.toml"):
        for block in re.findall(r"\[\[bin\]\]([^\[]*)", manifest.read_text()):
            m = re.search(r'name\s*=\s*"([^"]+)"', block)
            if m:
                names.add(m.group(1))
    return names


def sources(root):
    files = []
    for pattern in ("crates/*/tests/**/*.rs", "crates/*/src/**/*.rs", "crates/*/test-support/*.rs"):
        files.extend(root.glob(pattern))
    helper = (root / HELPER).resolve()
    return sorted({f for f in files if f.resolve() != helper})


def line_of(text, pos):
    return text.count("\n", 0, pos) + 1


def check(root):
    root = pathlib.Path(root)
    files = {path: path.read_text() for path in sources(root)}
    masks = {path: code_mask(text) for path, text in files.items()}

    # Constants that hold a binary path, wherever they are defined. Their uses
    # are checked like the env! call itself (contract tests include a shared
    # file that uses a constant its includer defines).
    constants = set()
    for path, (code, _) in masks.items():
        constants.update(re.findall(
            r"\b(?:const|static)\s+([A-Z_][A-Z0-9_]*)\s*:[^=;]*=\s*env!\(\s*\"CARGO_BIN_EXE_", code))
    names = binary_names(root)

    problems = []
    for path, text in files.items():
        code, in_string = masks[path]
        rel = path.relative_to(root)
        hits = []  # (position, how it is written)
        for m in re.finditer(r'env!\(\s*"CARGO_BIN_EXE_[^"]*"\s*\)', code):
            hits.append((m.start(), m.group(0)))
        for name in constants:
            for m in re.finditer(r"\b%s\b" % name, code):
                if not in_string[m.start()]:
                    hits.append((m.start(), name))
        for m in re.finditer(r'Command::new\(\s*"([^"]+)"', code):
            program = m.group(1)
            if program in names or any(program.endswith("/" + b) for b in names):
                hits.append((m.start() + len("Command::new("), f'"{program}"'))

        spans = list(functions(code, in_string))
        for pos, written in sorted(hits):
            before = code[:pos].rstrip()
            line = line_of(text, pos)
            enclosing = [s for s in spans if s[1] < pos < s[2]]
            if not enclosing:
                definition = re.search(r"\b(?:const|static)\s+[A-Z_][A-Z0-9_]*\s*:[^=;]*=\s*$", before)
                if definition or re.search(r"\b(?:const|static)\s+%s\b" % re.escape(written), code[max(0, pos - 20):pos + len(written)]):
                    continue
                problems.append(f"{rel}:{line}: {written} outside a function; hold it in a const or move it into a keyed function")
                continue
            fn_start, body_start, body_end = enclosing[-1]  # the innermost
            body = code[body_start:body_end]
            if marked(text, fn_start):
                if not any(call in text[body_start:body_end] for call in FENCE_CALLS):
                    problems.append(f"{rel}:{line_of(text, fn_start)}: marked {MARKER} but never fences (no fenced_tool or sandbox-exec)")
                continue
            if before.endswith("Command::new("):
                problems.append(f"{rel}:{line}: Command::new({written}) runs a tool with no test key; use keyed_tool({written})")
                continue
            if before.endswith("keyed_tool(") or before.endswith("fenced_tool("):
                continue
            if not any(call in body for call in HELPER_CALLS):
                problems.append(f"{rel}:{line}: {written} is used in a function that never calls with_test_key, keyed_tool or fenced_tool")
    # Each rule above only matters if the helper exists and still sets the key.
    helper = root / HELPER
    if helper.exists():
        helper_text = helper.read_text()
        for needed in ('"ETUDE_JOURNAL_KEY"', '"ETUDE_STATE_DIR"', '"HOME"', 'deny process-exec (literal "/usr/bin/security")'):
            if needed not in helper_text:
                problems.append(f"{HELPER}: no longer mentions {needed}")
    elif files:
        problems.append(f"{HELPER}: missing")
    return problems


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
