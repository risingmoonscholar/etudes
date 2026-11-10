#!/usr/bin/env python3
"""Build the Études site from site/ and the public docs.

    scripts/build-site.py              build into _site/
    scripts/build-site.py --out DIR    build into DIR

Everything under site/ is copied as it is (except the data the build reads:
tools.json, docs.json and changelog/), so the demo page, the release pages,
the GIFs and transcripts.json keep their URLs and stay byte-identical to what
check-claims.sh and check-transcripts-reproduce.sh check. Then each public doc
becomes /docs/<slug>/index.html in the docs layout, and the changelog is built:
/changelog/ lists the entries in site/changelog/*.json, newest first, and the
release pages that have none; /changelog/<tool>-<version>/ shows one entry.
The entries come from scripts/release-runs.py; their raw logs and fixtures
stay in the repo and are not published.

The home page, /index.html, is built: the tools from site/tools.json, the
install lines with versions read from each tool's Cargo.toml, What's new from
the newest entry in site/changelog/, and captured output from
site/transcripts.json. check-site.py renders it again and compares.

Which docs are public is data: site/tools.json (one entry per tool) and
site/docs.json (the shared pages). Nothing else from the repo goes on the site.

The Markdown converter below renders: ATX headings, paragraphs, flat bullet
and ordered lists (any start number), fenced code at the left margin, tables,
code spans, links (including a ")" inside a balanced URL), <https://...>
autolinks, * and _ emphasis, ** and __ strong, backslash escapes and HTML
entities. Everything else stops the build with the file and line: block
quotes, HTML blocks and inline HTML, images, reference links and definitions,
link titles, rules and underlined headings, nested lists, indented code, fences
that do not start at the left margin, and link schemes other than http, https
and mailto.

One rule changes content. A sentence "Back to the [README](path)." whose link
points at a repo file that is not on the site is navigation for readers on
GitHub; the site has its own, so the build leaves that sentence out. The page's
meta description is the first paragraph that remains.

Standard library only. Exit 0 built, 1 a doc or link could not be built,
2 the output folder could not be used.
"""
import html
import functools
import html.entities
import json
import posixpath
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SITE = ROOT / "site"
REPO_URL = "https://github.com/risingmoonscholar/etudes"
BLOB_URL = REPO_URL + "/blob/main/"
TREE_URL = REPO_URL + "/tree/main/"
# The full text of every change, for readers who want more than the entries.
CHANGELOG_FILE_URL = BLOB_URL + "CHANGELOG.md"
# Every built docs page carries this, so a rebuild can tell its own output
# folder from someone else's before it deletes anything.
GENERATOR = '<meta name="generator" content="scripts/build-site.py">'
# Read by the build, not by any page.
BUILD_ONLY = ("tools.json", "docs.json", ".DS_Store")
# Folders under site/ the build reads and does not publish as they are.
BUILD_ONLY_DIRS = ("changelog",)
SCHEMES = ("http", "https", "mailto")


class BuildError(Exception):
    pass


# --- data -------------------------------------------------------------------

def load_json(name):
    path = SITE / name
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise BuildError(f"site/{name} cannot be read: {error}") from error


def slug_for(source):
    return Path(source).stem.lower()


def public_pages():
    """Every public doc, in sidebar order, as dicts with source/slug/title/group."""
    tools = load_json("tools.json").get("tools")
    sections = load_json("docs.json").get("sections")
    if not isinstance(tools, list) or not tools:
        raise BuildError("site/tools.json lists no tools")
    if not isinstance(sections, list):
        raise BuildError("site/docs.json has no sections list")
    pages = []
    for tool in tools:
        for key in ("name", "description", "what", "docs"):
            if not isinstance(tool.get(key), str) or not tool[key].strip():
                raise BuildError(f"site/tools.json: a tool has no {key}: {tool!r}")
        pages.append({"source": tool["docs"], "title": tool["name"], "group": "Tools",
                      "tool": True, "description": tool["description"]})
    for section in sections:
        for page in section.get("pages", []):
            pages.append({"source": page["source"], "title": page["title"],
                          "group": section["title"], "tool": False})
    seen = set()
    for page in pages:
        if not (ROOT / page["source"]).is_file():
            raise BuildError(f"public doc {page['source']} does not exist")
        page["slug"] = slug_for(page["source"])
        if page["slug"] in seen:
            raise BuildError(f"two public docs share the URL docs/{page['slug']}/")
        seen.add(page["slug"])
        page["dir"] = f"docs/{page['slug']}"
    return pages


# --- links ------------------------------------------------------------------

def rel_dir(from_dir, to_dir):
    """A relative URL from one page folder to another, ending in '/'."""
    rel = posixpath.relpath(to_dir or ".", from_dir or ".")
    return "./" if rel == "." else rel + "/"


class Linker:
    def __init__(self, pages):
        self.by_source = {p["source"]: p for p in pages}

    def repo_path(self, source, target):
        """The repo path a relative link names, or None for a scheme or anchor."""
        if re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", target) or target.startswith("#"):
            return None
        path = target.partition("#")[0]
        return posixpath.normpath(posixpath.join(posixpath.dirname(source), path))

    def on_site(self, source, target):
        return self.repo_path(source, target) in self.by_source

    def resolve(self, source, page_dir, target):
        """Turn a link as written in SOURCE into a URL for the page at PAGE_DIR."""
        scheme = re.match(r"^([a-zA-Z][a-zA-Z0-9+.-]*):", target)
        if scheme:
            if scheme.group(1).lower() not in SCHEMES:
                raise BuildError(f"{source}: link {target!r} uses the scheme "
                                 f"{scheme.group(1)}:, which the site does not link to")
            return target
        if target.startswith("#"):
            return target
        frag = target.partition("#")[2]
        resolved = self.repo_path(source, target)
        if resolved.startswith("../") or resolved == "..":
            raise BuildError(f"{source}: link {target!r} leaves the repository")
        suffix = f"#{frag}" if frag else ""
        if resolved in self.by_source:
            return rel_dir(page_dir, self.by_source[resolved]["dir"]) + suffix
        on_disk = ROOT / resolved
        if on_disk.is_dir():
            return TREE_URL + resolved + suffix
        if on_disk.is_file():
            return BLOB_URL + resolved + suffix
        raise BuildError(f"{source}: link {target!r} points at {resolved}, which is not in the repository")


# --- markdown ---------------------------------------------------------------

ENTITY = re.compile(r"&(#[0-9]{1,7};|#[xX][0-9a-fA-F]{1,6};|[A-Za-z][A-Za-z0-9]{1,31};)")


def esc(text):
    """Escape for HTML text, keeping entity references the author wrote."""
    out, pos = [], 0
    for m in ENTITY.finditer(text):
        ref = m.group(1)
        if ref[0] != "#" and ref not in html.entities.html5:
            continue
        out.append(html.escape(text[pos:m.start()], quote=False))
        out.append("&" + ref)
        pos = m.end()
    out.append(html.escape(text[pos:], quote=False))
    return "".join(out)


def esc_attr(text):
    return html.escape(text, quote=True)


def heading_id(text):
    """GitHub's anchor rule, so links written for GitHub land here too."""
    slug = text.strip().lower()
    slug = re.sub(r"[^\w\- ]", "", slug)
    return slug.replace(" ", "-")


class Ids:
    """Unique, non-empty anchors for one page."""

    def __init__(self):
        self.used = set()

    def make(self, text):
        base = heading_id(text) or "section"
        candidate, n = base, 0
        while candidate in self.used:
            n += 1
            candidate = f"{base}-{n}"
        self.used.add(candidate)
        return candidate


PUNCT = r"!\"#$%&'()*+,\-./:;<=>?@\[\\\]^_`{|}~"
# Leftmost of: a code span, a backslash escape, an autolink, or anything that
# looks like inline HTML.
INLINE_TOKEN = re.compile(
    r"(?P<code>(?P<ticks>`+)(?P<body>.+?)(?<!`)(?P=ticks)(?!`))"
    rf"|\\(?P<escaped>[{PUNCT}])"
    r"|<(?P<auto>(?:https?|mailto):[^\s<>]+)>"
    r"|(?P<html></?[A-Za-z][A-Za-z0-9-]*(?:\s[^<>]*)?/?>|<!--|<\?|<![A-Za-z])",
    re.S,
)
STRONG = re.compile(r"\*\*(?=\S)(.+?)(?<=\S)\*\*|(?<!\w)__(?=\S)(.+?)(?<=\S)__(?!\w)", re.S)
EM = re.compile(r"(?<![\w*])\*(?=[^\s*])(.+?)(?<=[^\s*])\*(?![\w*])"
                r"|(?<![\w_])_(?=[^\s_])(.+?)(?<=[^\s_])_(?![\w_])", re.S)


def emphasis(text):
    text = STRONG.sub(lambda m: f"<strong>{m.group(1) or m.group(2)}</strong>", text)
    return EM.sub(lambda m: f"<em>{m.group(1) or m.group(2)}</em>", text)


def find_link(text, start=0):
    """The next [text](dest) at or after START: (begin, end, label, dest) or None.

    The destination may hold balanced parentheses, as in a Wikipedia URL.
    """
    i = text.find("[", start)
    while i >= 0:
        depth, j = 0, i
        while j < len(text):
            if text[j] == "\\":
                j += 2
                continue
            if text[j] == "[":
                depth += 1
            elif text[j] == "]":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        if j >= len(text):
            return None
        if j + 1 < len(text) and text[j + 1] == "(":
            k, parens = j + 2, 1
            while k < len(text):
                if text[k] == "\\":
                    k += 2
                    continue
                if text[k] == "(":
                    parens += 1
                elif text[k] == ")":
                    parens -= 1
                    if parens == 0:
                        return i, k + 1, text[i + 1:j], text[j + 2:k]
                k += 1
            return None
        i = text.find("[", i + 1)
    return None


def plain(text):
    """Text with the inline markup removed, for anchors, the TOC and descriptions."""
    text = INLINE_TOKEN.sub(lambda m: (m.group("body") or "").strip() if m.group("code")
                            else m.group("escaped") or m.group("auto") or "", text)
    out, pos = [], 0
    while (link := find_link(text, pos)):
        begin, end, label, _ = link
        out.append(text[pos:begin] + label)
        pos = end
    out.append(text[pos:])
    text = "".join(out)
    text = STRONG.sub(lambda m: m.group(1) or m.group(2), text)
    text = EM.sub(lambda m: m.group(1) or m.group(2), text)
    return html.unescape(text)


# A trailing "Back to the [README](../README.md)." sentence.
BACK_LINK = re.compile(r"(?:^|(?<=[.!?])\s+)Back to the \[[^\]]+\]\((?P<dest>[^)\s]+)\)\.\s*$")


class Converter:
    FENCE = re.compile(r"^(```+|~~~+)\s*([\w+-]*)\s*$")
    HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
    ITEM = re.compile(r"^([-*+]|\d{1,9}[.)])(\s+)(.*)$")
    TABLE_SEP = re.compile(r"^\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?\s*$")

    def __init__(self, source, page_dir, linker):
        self.source = source
        self.page_dir = page_dir
        self.linker = linker
        self.ids = Ids()
        self.headings = []
        self.title = None
        self.description = None

    def fail(self, lineno, why):
        raise BuildError(f"{self.source}:{lineno}: {why}")

    # -- inline --

    def inline(self, text, lineno):
        kept = []

        def keep(fragment):
            kept.append(fragment)
            return f"\x00{len(kept) - 1}\x00"

        def token(m):
            if m.group("code"):
                body = m.group("body").replace("\n", " ")
                if body.startswith(" ") and body.endswith(" ") and body.strip():
                    body = body[1:-1]
                return keep(f"<code>{html.escape(body, quote=False)}</code>")
            if m.group("escaped"):
                return keep(html.escape(m.group("escaped"), quote=False))
            if m.group("auto"):
                href = self.linker.resolve(self.source, self.page_dir, m.group("auto"))
                return keep(f'<a href="{esc_attr(href)}">{esc(m.group("auto"))}</a>')
            self.fail(lineno, f"inline HTML {m.group('html')!r} is not supported by the site build")

        text = INLINE_TOKEN.sub(token, text)

        out, pos = [], 0
        while (link := find_link(text, pos)):
            begin, end, label, dest = link
            if begin > 0 and text[begin - 1] == "!":
                self.fail(lineno, "images are not supported by the site build")
            if re.search(r"\s", dest):
                self.fail(lineno, f"link {dest!r} has a title or a space, which the site build does not support")
            href = self.linker.resolve(self.source, self.page_dir, dest)
            out.append(text[pos:begin])
            out.append(keep(f'<a href="{esc_attr(href)}">{emphasis(esc(label))}</a>'))
            pos = end
        out.append(text[pos:])
        text = "".join(out)

        if re.search(r"!\[|\]\[[^\]]*\]|\[[^\]]+\]\[", text):
            self.fail(lineno, "images and reference links are not supported by the site build")

        rendered = emphasis(esc(text))
        while "\x00" in rendered:
            rendered = re.sub(r"\x00(\d+)\x00", lambda m: kept[int(m.group(1))], rendered)
        return rendered

    # -- blocks --

    def unsupported(self, line, lineno):
        stripped = line.strip()
        if stripped.startswith(">"):
            self.fail(lineno, "block quotes are not supported by the site build")
        if re.match(r"^<[a-zA-Z/!?]", stripped) and not re.match(r"^<(https?|mailto):", stripped):
            self.fail(lineno, "raw HTML blocks are not supported by the site build")
        if re.match(r"^(\*\s*){3,}$|^(-\s*){3,}$|^(_\s*){3,}$|^=+$", stripped):
            self.fail(lineno, "rules and underlined headings are not supported by the site build")
        if re.match(r"^\[[^\]]+\]:\s", stripped):
            self.fail(lineno, "reference link definitions are not supported by the site build")
        if stripped.startswith("!["):
            self.fail(lineno, "images are not supported by the site build")
        if line != line.lstrip() and self.FENCE.match(stripped):
            self.fail(lineno, "a code fence that does not start at the left margin "
                              "(for example inside a list item) is not supported by the site build")

    def starts_block(self, line):
        return bool(self.FENCE.match(line) or self.HEADING.match(line)
                    or re.match(r"^([-*+]|1[.)])\s+", line) or line.startswith("|"))

    def paragraph(self, text, lineno):
        """Render one paragraph, leaving out a GitHub-only back link."""
        back = BACK_LINK.search(text)
        if back and not self.linker.on_site(self.source, back.group("dest")):
            text = text[:back.start()].rstrip()
        if not text.strip():
            return None
        if self.description is None:
            self.description = re.sub(r"\s+", " ", plain(text)).strip()
        return f"<p>{self.inline(text, lineno)}</p>"

    def convert(self, text):
        lines = text.split("\n")
        out = []
        i = 0
        while i < len(lines):
            line = lines[i]
            lineno = i + 1
            if not line.strip():
                i += 1
                continue
            if line.startswith("    ") or line.startswith("\t"):
                self.fail(lineno, "indented code blocks are not supported by the site build; use a fence")
            fence = self.FENCE.match(line)
            if fence:
                marker, lang = fence.group(1), fence.group(2)
                body = []
                i += 1
                while i < len(lines) and not lines[i].startswith(marker):
                    body.append(lines[i])
                    i += 1
                if i >= len(lines):
                    self.fail(lineno, "code fence is never closed")
                i += 1
                cls = f' class="language-{esc_attr(lang)}"' if lang else ""
                out.append(f"<pre><code{cls}>{html.escape(chr(10).join(body), quote=False)}</code></pre>")
                continue
            heading = self.HEADING.match(line)
            if heading:
                level = len(heading.group(1))
                raw = heading.group(2)
                text_plain = plain(raw)
                hid = self.ids.make(text_plain)
                inner = self.inline(raw, lineno)
                if level == 1 and self.title is None:
                    self.title = text_plain.strip()
                    out.append(f'<h1 id="{hid}">{inner}</h1>')
                else:
                    if level in (2, 3):
                        self.headings.append((level, hid, inner))
                    out.append(f'<h{level} id="{hid}"><a class="anchor" href="#{hid}">{inner}</a></h{level}>')
                i += 1
                continue
            if line.startswith("|") and i + 1 < len(lines) and self.TABLE_SEP.match(lines[i + 1]):
                i = self.table(lines, i, out)
                continue
            if self.ITEM.match(line):
                i = self.list_block(lines, i, out)
                continue
            if line.startswith(" ") and self.ITEM.match(line.lstrip()):
                self.fail(lineno, "nested or indented lists are not supported by the site build")
            self.unsupported(line, lineno)
            para = [line]
            i += 1
            while i < len(lines) and lines[i].strip() and not self.starts_block(lines[i]):
                self.unsupported(lines[i], i + 1)
                para.append(lines[i])
                i += 1
            rendered = self.paragraph(chr(10).join(para), lineno)
            if rendered:
                out.append(rendered)
        return "\n".join(out)

    @staticmethod
    def split_row(line):
        line = line.strip()
        if line.startswith("|"):
            line = line[1:]
        if line.endswith("|") and not line.endswith("\\|"):
            line = line[:-1]
        cells, cur, in_code = [], [], False
        for ch in line:
            if ch == "`":
                in_code = not in_code
            if ch == "|" and not in_code and not (cur and cur[-1] == "\\"):
                cells.append("".join(cur).strip())
                cur = []
                continue
            cur.append(ch)
        cells.append("".join(cur).strip())
        return cells

    def table(self, lines, i, out):
        head = self.split_row(lines[i])
        rows = []
        j = i + 2
        while j < len(lines) and lines[j].startswith("|"):
            row = self.split_row(lines[j])
            if len(row) != len(head):
                self.fail(j + 1, f"table row has {len(row)} cells; the header has {len(head)}")
            rows.append((j + 1, row))
            j += 1
        parts = ['<div class="table-wrap"><table><thead><tr>']
        parts += [f"<th>{self.inline(c, i + 1)}</th>" for c in head]
        parts.append("</tr></thead><tbody>")
        for lineno, row in rows:
            parts.append("<tr>" + "".join(f"<td>{self.inline(c, lineno)}</td>" for c in row) + "</tr>")
        parts.append("</tbody></table></div>")
        out.append("".join(parts))
        return j

    def list_block(self, lines, i, out):
        first = self.ITEM.match(lines[i])
        ordered = first.group(1)[0].isdigit()
        start = int(first.group(1)[:-1]) if ordered else 1
        items = []  # (lineno, [paragraph lines] per paragraph)
        while i < len(lines):
            m = self.ITEM.match(lines[i])
            if not m or m.group(1)[0].isdigit() != ordered:
                break
            lineno = i + 1
            content_indent = len(m.group(1)) + len(m.group(2))
            if len(m.group(2)) > 4:
                self.fail(lineno, "a list item that starts with indented code is not supported by the site build")
            self.unsupported(m.group(3), lineno)
            if self.FENCE.match(m.group(3)):
                self.fail(lineno, "a code fence inside a list item is not supported by the site build")
            paras = [[m.group(3)]]
            i += 1
            while i < len(lines):
                line = lines[i]
                if not line.strip():
                    # A blank line: the list goes on if the next text is a new
                    # item, or an indented paragraph of this one.
                    k = i
                    while k < len(lines) and not lines[k].strip():
                        k += 1
                    if k < len(lines) and lines[k].startswith("  ") and not self.ITEM.match(lines[k]):
                        indent = len(lines[k]) - len(lines[k].lstrip())
                        if self.ITEM.match(lines[k].lstrip()):
                            self.fail(k + 1, "nested lists are not supported by the site build")
                        if indent >= content_indent + 4:
                            self.fail(k + 1, "indented code inside a list item is not supported by the site build")
                        self.unsupported(lines[k], k + 1)
                        paras.append([])
                        i = k
                        continue
                    i = k if k < len(lines) and self.ITEM.match(lines[k]) else i
                    break
                if self.ITEM.match(line):
                    break
                if line.startswith(" ") and self.ITEM.match(line.lstrip()):
                    self.fail(i + 1, "nested lists are not supported by the site build")
                if not line.startswith(" ") and self.starts_block(line):
                    break
                self.unsupported(line, i + 1)
                paras[-1].append(line.strip())
                i += 1
            items.append((lineno, paras))
            if i < len(lines) and not lines[i].strip():
                break
        tag = "ol" if ordered else "ul"
        start_attr = f' start="{start}"' if ordered and start != 1 else ""
        parts = [f"<{tag}{start_attr}>"]
        for lineno, paras in items:
            rendered = [self.inline("\n".join(p), lineno) for p in paras if p]
            if len(rendered) == 1:
                parts.append(f"<li>{rendered[0]}</li>")
            else:
                parts.append("<li>" + "".join(f"<p>{r}</p>" for r in rendered) + "</li>")
        parts.append(f"</{tag}>")
        out.append("\n".join(parts))
        return i


# --- layout -----------------------------------------------------------------

def topbar(root, current):
    """The bar every page shares. CURRENT is "home", "docs" or "changelog"."""
    brand_cur = ' aria-current="page"' if current == "home" else ""
    docs_cur = ' aria-current="true"' if current == "docs" else ""
    log_cur = ' aria-current="true"' if current == "changelog" else ""
    return (f'<a class="skip" href="#main">Skip to content</a>\n'
            f'<header class="topbar">\n'
            f'<a class="brand" href="{root}"{brand_cur}>Études</a>\n'
            f'<nav class="topnav" aria-label="Site">\n'
            f'<a href="{root}docs/"{docs_cur}>Docs</a>\n'
            f'<a href="{root}changelog/"{log_cur}>Changelog</a>\n'
            f'<a href="{REPO_URL}">GitHub</a>\n'
            f'</nav>\n'
            f'</header>')


def sidebar(pages, current_dir, first=None):
    """The doc tree. FIRST: (title, [(dir, label)]) groups shown above it."""
    def link(href_dir, label, mono=False):
        href = rel_dir(current_dir, href_dir)
        cur = ' aria-current="page"' if href_dir == current_dir else ""
        text = f"<code>{esc(label)}</code>" if mono else esc(label)
        return f'<li><a href="{esc_attr(href)}"{cur}>{text}</a></li>'

    groups = [(title, [link(d, label) for d, label in items]) for title, items in first or []]
    groups.append(("Get started", [link("docs", "Overview")]))
    order = []
    by_group = {}
    for page in pages:
        if page["group"] not in by_group:
            by_group[page["group"]] = []
            order.append(page["group"])
        by_group[page["group"]].append(link(page["dir"], page["title"], page["tool"]))
    groups += [(g, by_group[g]) for g in order]
    return "\n".join(
        f'<div class="navgroup"><h2>{esc(title)}</h2><ul>{"".join(items)}</ul></div>'
        for title, items in groups
    )


def toc(headings):
    if not headings:
        return ""
    items = "".join(
        f'<li class="l{level}"><a href="#{hid}">{re.sub(r"<[^>]+>", "", inner)}</a></li>'
        for level, hid, inner in headings
    )
    return f'<aside class="toc" aria-label="On this page"><h2>On this page</h2><ul>{items}</ul></aside>'


def layout(*, title, description, page_dir, pages, content, headings, current="docs",
           first=None, body_class=None):
    root = rel_dir(page_dir, "")
    nav = sidebar(pages, page_dir, first)
    body = f' class="{body_class}"' if body_class else ""
    right = toc(headings)
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{esc(title)} · Études</title>
<meta name="description" content="{esc_attr(description)}">
<meta name="color-scheme" content="light dark">
{GENERATOR}
<link rel="stylesheet" href="{root}assets/site.css">
<link rel="icon" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><text y='13' font-size='13'>&#9679;</text></svg>">
</head>
<body{body}>
{topbar(root, current)}
<div class="layout">
<details class="menu"><summary>Menu</summary><nav aria-label="Docs menu">
{nav}
</nav></details>
<aside class="sidebar" aria-label="Docs">
<nav>
{nav}
</nav>
</aside>
<main class="content" id="main">
{content}
</main>
{right}
</div>
</body>
</html>
"""


# --- home -------------------------------------------------------------------

# The home page is built, not copied: its install lines carry versions read
# from each tool's Cargo.toml, its "What's new" comes from the newest entry in
# site/changelog/, and its tool blocks come from site/tools.json and the
# captured runs in site/transcripts.json. check-site.py renders it again and
# refuses a published home page that differs.

def tool_versions(tools):
    """{tool: (package name, version)} from each tool's Cargo.toml."""
    versions = {}
    for tool in tools:
        manifest = ROOT / tool.get("crate", "") / "Cargo.toml"
        try:
            package = tomllib.loads(manifest.read_text(encoding="utf-8"))["package"]
            versions[tool["name"]] = (package["name"], package["version"])
        except (OSError, KeyError, tomllib.TOMLDecodeError) as error:
            raise BuildError(f"site/tools.json: {tool['name']}: cannot read the version from "
                             f"{tool.get('crate', '(no crate)')}/Cargo.toml: {error}") from error
    return versions


def install_line(tool, package, version):
    return f"cargo install --locked --git {REPO_URL} --tag {tool}-v{version} {package}"


# --- release state, from git ---------------------------------------------------
#
# An entry does not say whether its release is tagged, or when: that changes
# after the runs (the tag comes later) and git is the record of it. So the
# build asks git, every time:
#   - no <tool>-v<version> tag: a release candidate, no date;
#   - the tag names the commit the after build ran at: tagged, the tag's date;
#   - the tag names another commit whose source for that tool is the same:
#     tagged, and the footer says the run's build had the same source. "Same"
#     means `git diff` finds no change between the two commits in the tool's
#     crate, the crates it depends on by path (read from both commits'
#     Cargo.toml files: [dependencies], [build-dependencies], their
#     [target.*] forms, and path dependencies inherited from the workspace),
#     the workspace Cargo.toml and Cargo.lock, and the toolchain and Cargo
#     settings (rust-toolchain.toml, rust-toolchain, .cargo/config.toml,
#     .cargo/config). Those are the source and settings
#     `cargo build --release --locked -p <tool>-cli` reads, so the tag holds
#     the same source the entry's build was made from. This compares source,
#     not bytes: the compiler on the machine is not part of it. A change
#     anywhere else (another tool, docs, the site) does not count;
#   - the tag names a commit with different source: the build stops, because
#     the entry no longer shows that release. Run scripts/release-runs.py on
#     the tag.

def git_out(*args):
    done = subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True)
    return done.stdout.strip() if done.returncode == 0 else None


# Files outside the crates that change what cargo builds.
BUILD_SETTINGS = ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "rust-toolchain",
                  ".cargo/config.toml", ".cargo/config")
DEP_TABLES = ("dependencies", "build-dependencies")


def crate_graph(read, crate):
    """The crate folder and every crate it depends on by path.

    READ(path) returns a file's text, or None. Covers [dependencies] and
    [build-dependencies], the same tables under [target.<cfg>], and
    `dep = { workspace = true }` entries whose [workspace.dependencies] entry
    in the root Cargo.toml has a path (relative to the workspace root).
    """
    root_text = read("Cargo.toml")
    workspace = (tomllib.loads(root_text).get("workspace", {}).get("dependencies", {})
                 if root_text else {})
    seen = set()

    def walk(folder):
        if folder in seen:
            return
        seen.add(folder)
        text = read(f"{folder}/Cargo.toml")
        if text is None:
            return
        manifest = tomllib.loads(text)
        tables = [manifest.get(t, {}) for t in DEP_TABLES]
        for target in manifest.get("target", {}).values():
            tables += [target.get(t, {}) for t in DEP_TABLES]
        for table in tables:
            for name, dep in table.items():
                if not isinstance(dep, dict):
                    continue
                if "path" in dep:
                    walk(posixpath.normpath(posixpath.join(folder, dep["path"])))
                elif dep.get("workspace") is True:
                    inherited = workspace.get(dep.get("package", name), workspace.get(name))
                    if isinstance(inherited, dict) and "path" in inherited:
                        walk(posixpath.normpath(inherited["path"]))

    walk(crate)
    return seen


def crate_paths(rev, crate):
    """crate_graph() as the files are at commit REV."""
    return crate_graph(lambda path: git_out("show", f"{rev}:{path}"), crate)


def same_tool_source(tool, rev_a, rev_b):
    paths = sorted(crate_paths(rev_a, f"crates/{tool}-cli") | crate_paths(rev_b, f"crates/{tool}-cli"))
    done = subprocess.run(["git", "diff", "--quiet", rev_a, rev_b, "--", *paths, *BUILD_SETTINGS],
                          cwd=ROOT, capture_output=True, text=True)
    if done.returncode not in (0, 1):
        raise BuildError(f"cannot compare {rev_a[:7]} with {rev_b[:7]}: {done.stderr.strip()}. "
                         "Fetch the full history and tags (git fetch --tags --unshallow).")
    return done.returncode == 0


@functools.lru_cache(maxsize=None)
def _release_state(tool, version, rev):
    tag = f"{tool}-v{version}"
    tag_rev = git_out("rev-parse", "--verify", "--quiet", f"refs/tags/{tag}^{{commit}}")
    if not tag_rev:
        return {"tagged": False, "tag": tag}
    date = git_out("for-each-ref", "--format=%(creatordate:short)", f"refs/tags/{tag}")
    if tag_rev != rev and not same_tool_source(tool, rev, tag_rev):
        raise BuildError(f"the tag {tag} names {tag_rev[:7]}, whose {tool} source differs from "
                         f"commit {rev[:7]}, where the entry's after build ran. Run "
                         f"scripts/release-runs.py {tool} {version} --before <previous tag> --after {tag}")
    return {"tagged": True, "tag": tag, "tag_commit": tag_rev[:7], "date": date, "same_rev": tag_rev == rev}


def release_state(entry):
    """{"tagged", "tag", and when tagged "tag_commit", "date", "same_rev"}, from git."""
    return dict(_release_state(entry["tool"], entry["version"], entry["builds"]["after"]["rev"]))


def footer(entry):
    state = release_state(entry)
    before, after = entry["builds"]["before"], entry["builds"]["after"]
    before_label = (f"tag {before['tag']} (commit {before['commit']})" if before.get("tag")
                    else f"commit {before['commit']}")
    if not state["tagged"]:
        after_label = f"the {entry['version']} release candidate at commit {after['commit']}, not tagged yet"
    elif state["same_rev"]:
        after_label = f"tag {state['tag']} (commit {after['commit']})"
    else:
        after_label = (f"tag {state['tag']} (commit {state['tag_commit']}), built and run at commit "
                       f"{after['commit']}, which has the same {entry['tool']} source")
    return f"Both outputs come from real runs of two builds: {before_label} and {after_label}. {entry['runs_note']}"


def release_meta(entry):
    """The date of a tagged release, or the release-candidate mark."""
    state = release_state(entry)
    if state["tagged"]:
        return f'<time datetime="{esc_attr(state["date"])}">{esc(state["date"])}</time>'
    return '<span class="pill rc">Release candidate</span>'


def entry_order(entry):
    """Newest first sorts by this, descending: candidates, then tag date, then version."""
    state = release_state(entry)
    return (not state["tagged"], state.get("date", ""), tuple(int(n) for n in entry["version"].split(".")))


def changelog_entries():
    """Every release entry in site/changelog/, newest first (by date, then version)."""
    entries = []
    for path in sorted((SITE / "changelog").glob("*.json")):
        try:
            entry = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise BuildError(f"site/changelog/{path.name} cannot be read: {error}") from error
        for key in ("tool", "version", "label", "headline", "lead",
                    "before", "after", "next", "builds", "runs_note"):
            if key not in entry:
                raise BuildError(f"site/changelog/{path.name} has no {key}")
        # Written by hand after scripts/release-runs.py made the entry.
        for key in ("label", "headline", "lead"):
            if not entry[key]:
                raise BuildError(f"site/changelog/{path.name}: {key} is empty; write it by hand")
        if not entry["next"].get("note"):
            raise BuildError(f"site/changelog/{path.name}: next.note is empty; write it by hand")
        if path.stem != f"{entry['tool']}-{entry['version']}":
            raise BuildError(f"site/changelog/{path.name} holds {entry['tool']} {entry['version']}; "
                             f"name it {entry['tool']}-{entry['version']}.json")
        entries.append(entry)
    if not entries:
        raise BuildError("site/changelog/ holds no release entry for What's new")
    return sorted(entries, key=entry_order, reverse=True)


def latest_entry():
    """The newest release entry: the one What's new shows."""
    return changelog_entries()[0]


def code_text(text):
    """Plain data text where `backticks` mark code, as HTML."""
    pieces = text.split("`")
    if len(pieces) % 2 == 0:
        raise BuildError(f"site/tools.json: unmatched backtick in {text!r}")
    return "".join(f"<code>{esc(piece)}</code>" if n % 2 else esc(piece)
                   for n, piece in enumerate(pieces))


def rich(parts):
    """Text parts as written in a changelog entry: {"text": ...} or {"code": ...}."""
    out = []
    for part in parts:
        if "code" in part:
            out.append(f"<code>{html.escape(part['code'], quote=False)}</code>")
        else:
            out.append(html.escape(part["text"], quote=False))
    return "".join(out)


def json_field(field):
    """One key of a --json run, as a shell comment so it never reads as output."""
    if not field["present"]:
        text = f'# --json has no {field["path"]} key'
    else:
        text = f'# --json {field["path"]}: {json.dumps(field["value"])}'
    return f'<span class="jsonfield">{html.escape(text, quote=False)}</span>'


def terminal(runs):
    """Commands, their output lines and exit codes, as one block."""
    blocks = []
    for run in runs:
        lines = [f'<span class="prompt">$ </span>{html.escape(run["command"], quote=False)}']
        lines += [html.escape(line, quote=False) for line in run["output"]]
        lines += [json_field(field) for field in run.get("fields", [])]
        lines.append(f'<span class="exitline">exit {int(run["exit"])}</span>')
        blocks.append("\n".join(lines))
    return '<pre class="term"><code>' + "\n\n".join(blocks) + "</code></pre>"


def entry_side(side, data):
    title = "Before" if side == "before" else "After"
    files = "".join(f"<li><code>{esc(name)}</code></li>" for name in data["result"]["files"])
    files = f"<ul>{files}</ul>" if files else ""
    return (f'<div class="side {side}">'
            f'<div class="side-head"><span class="who"><span class="dot" aria-hidden="true"></span>{title}</span>'
            f'<span class="build">{esc(data["build"])}</span></div>'
            f'{terminal(data["runs"])}'
            f'<div class="result"><p class="result-label">{esc(data["result"]["label"])}</p>'
            f'{files}</div>'
            f'<p class="side-exit">exit code <strong>{int(data["exit"])}</strong></p>'
            f'</div>')


def render_entry(entry, level=3):
    nxt = entry["next"]
    return (f'<article class="entry" aria-labelledby="entry-title">'
            f'<p class="entry-meta"><span class="entry-ver">{esc(entry["tool"])} {esc(entry["version"])}</span>'
            f'<span class="pill">{esc(entry["label"])}</span>{release_meta(entry)}</p>'
            f'<h{level} id="entry-title" class="entry-title">{esc(entry["headline"])}</h{level}>'
            f'<p class="entry-lead">{rich(entry["lead"])}</p>'
            f'<div class="compare">{entry_side("before", entry["before"])}{entry_side("after", entry["after"])}</div>'
            f'<p class="entry-note">{rich(nxt["note"])}</p>'
            f'{terminal([nxt])}'
            f'<p class="entry-foot">{esc(footer(entry))}</p>'
            f'</article>')


def copyable(text, label):
    return (f'<div class="cmd"><pre><code>{html.escape(text, quote=False)}</code></pre>'
            f'<button type="button" class="copy" data-copy="{esc_attr(text)}" '
            f'aria-label="Copy {esc_attr(label)}" hidden>Copy</button></div>')


def install_block(tools, versions):
    tabs = [("all", f"All {NUMBER_WORDS.get(len(tools), str(len(tools)))}",
             [install_line(t["name"], *versions[t["name"]]) for t in tools])]
    tabs += [(t["name"], t["name"], [install_line(t["name"], *versions[t["name"]])]) for t in tools]
    buttons = "".join(
        f'<button type="button" role="tab" id="tab-{key}" aria-controls="install-{key}" '
        f'aria-selected="{"true" if n == 0 else "false"}" tabindex="{0 if n == 0 else -1}">{esc(label)}</button>'
        for n, (key, label, _) in enumerate(tabs))
    panels = "".join(
        f'<section class="tabpanel" id="install-{key}" aria-labelledby="label-{key}">'
        f'<h3 class="panel-label" id="label-{key}">{esc(label)}</h3>'
        f'{copyable(chr(10).join(lines), "the install " + ("commands" if len(lines) > 1 else "command") + " for " + label)}'
        f'</section>'
        for key, label, lines in tabs)
    return (f'<div class="install" data-tabs>'
            f'<div class="tablist" role="tablist" aria-label="Which tools to install" hidden>{buttons}</div>'
            f'{panels}</div>')


NUMBER_WORDS = {2: "two", 3: "three", 4: "four", 5: "five", 6: "six"}


def tool_block(tool, transcripts, root):
    name = tool["name"]
    items = []
    for command in tool.get("commands", []):
        out = ""
        label = command.get("transcript")
        if label:
            captured = transcripts.get(label)
            if captured is None:
                raise BuildError(f"site/tools.json: {name}: no captured run {label!r} in site/transcripts.json")
            if captured["command"] != command["command"]:
                raise BuildError(f"site/tools.json: {name}: {command['command']!r} is not the command "
                                 f"captured as {label!r} ({captured['command']!r})")
            out = (f'<pre class="term out" data-transcript="{esc_attr(label)}"><code>'
                   f'{html.escape(captured["output"].rstrip(), quote=False)}</code></pre>'
                   f'<p class="out-note">Captured output, exit {int(captured["exit"])}. '
                   f'The fixture folder is shown as <code>~/Desktop</code>.</p>')
        items.append(f'<li><pre class="line"><code><span class="prompt">$ </span>'
                     f'{html.escape(command["command"], quote=False)}</code></pre>'
                     f'<p>{code_text(command["says"])}</p>{out}</li>')
    docs_dir = f"docs/{slug_for(tool['docs'])}/"
    links = [f'<a href="{root}{docs_dir}">Read the {esc(name)} docs</a>']
    if tool.get("demo"):
        links.append(f'<a href="{root}{esc_attr(tool["demo"])}">See it run</a>')
    return (f'<section class="tool" id="{esc_attr(name)}" aria-labelledby="tool-{esc_attr(name)}">'
            f'<h3 id="tool-{esc_attr(name)}"><code>{esc(name)}</code></h3>'
            f'<p class="tool-desc">{code_text(tool["what"])}</p>'
            f'<ol class="commands">{"".join(items)}</ol>'
            f'<p class="tool-links">{" ".join(links)}</p></section>')


# The docs index says the same, in one sentence.
TAGLINE = ["Every move is checked first.", "Every failure says why."]

HOME_SCRIPT = """
(function () {
  document.documentElement.classList.add("js");
  // Install tabs. Without JavaScript every panel shows, each under its own label.
  document.querySelectorAll("[data-tabs]").forEach(function (box) {
    var list = box.querySelector("[role=tablist]");
    var tabs = Array.prototype.slice.call(list.querySelectorAll("[role=tab]"));
    function select(tab, focus) {
      tabs.forEach(function (t) {
        var on = t === tab;
        t.setAttribute("aria-selected", on ? "true" : "false");
        t.tabIndex = on ? 0 : -1;
        document.getElementById(t.getAttribute("aria-controls")).hidden = !on;
      });
      if (focus) tab.focus();
    }
    tabs.forEach(function (tab, i) {
      var panel = document.getElementById(tab.getAttribute("aria-controls"));
      panel.setAttribute("role", "tabpanel");
      panel.setAttribute("aria-labelledby", tab.id);
      panel.tabIndex = 0;
      tab.addEventListener("click", function () { select(tab, false); });
      tab.addEventListener("keydown", function (e) {
        var n = null;
        if (e.key === "ArrowRight") n = (i + 1) % tabs.length;
        else if (e.key === "ArrowLeft") n = (i - 1 + tabs.length) % tabs.length;
        else if (e.key === "Home") n = 0;
        else if (e.key === "End") n = tabs.length - 1;
        if (n !== null) { e.preventDefault(); select(tabs[n], true); }
      });
    });
    list.hidden = false;
    box.classList.add("tabbed");
    select(tabs[0], false);
  });
  // Copy buttons. Shown only where the browser can copy; the text stays selectable.
  if (navigator.clipboard && window.isSecureContext) {
    var status = document.getElementById("copy-status");
    document.querySelectorAll("button.copy").forEach(function (button) {
      button.hidden = false;
      button.addEventListener("click", function () {
        navigator.clipboard.writeText(button.getAttribute("data-copy")).then(function () {
          button.textContent = "Copied";
          status.textContent = "Copied to the clipboard.";
        }, function () {
          button.textContent = "Copy failed";
          status.textContent = "Copy failed. Select the text and copy it instead.";
        });
        setTimeout(function () { button.textContent = "Copy"; status.textContent = ""; }, 2000);
      });
    });
  }
  // The tagline: each word goes from muted to full as it crosses the middle of
  // the screen. Without JavaScript, or with reduced motion, it is shown in full.
  var words = document.querySelectorAll(".tagline .w");
  if (!words.length) return;
  var still = window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (still || !("IntersectionObserver" in window)) return;
  // Dim the words only once the observer has reported: if it never runs,
  // the tagline stays at full strength.
  var seen = new IntersectionObserver(function (entries) {
    document.documentElement.classList.add("reveal");
    entries.forEach(function (entry) {
      if (entry.isIntersecting) { entry.target.classList.add("on"); seen.unobserve(entry.target); }
    });
  }, { rootMargin: "0px 0px -45% 0px" });
  words.forEach(function (w, i) { w.style.transitionDelay = (i % 8) * 40 + "ms"; seen.observe(w); });
})();
"""


def build_home(pages):
    tools = load_json("tools.json")["tools"]
    versions = tool_versions(tools)
    captured = {t["label"]: t for t in load_json("transcripts.json").get("transcripts", [])}
    entry = latest_entry()
    root = "./"
    count = NUMBER_WORDS.get(len(tools), str(len(tools)))
    cards = "".join(
        f'<li><a class="toolcard" href="docs/{slug_for(t["docs"])}/">'
        f'<span class="name">{esc(t["name"])}</span>'
        f'<span class="desc">{code_text(t["what"])}</span>'
        f'<span class="more">{esc(t["name"])} docs</span></a></li>'
        for t in tools)
    checks = "\n".join(f"{t['name']} --version" for t in tools)
    first = tools[0]["name"]
    update = install_line(first, versions[first][0], "X.Y.Z")
    tagline = "<br>".join(
        " ".join(f'<span class="w">{esc(word)}</span>' for word in line.split()) for line in TAGLINE)
    blocks = "".join(tool_block(t, captured, root) for t in tools)
    entry_tag = f'{entry["tool"]}-v{entry["version"]}'
    more = [f'<a href="changelog/{entry_slug(entry)}/">The {esc(entry["tool"])} {esc(entry["version"])} '
            f'entry on its own page</a>']
    if (SITE / "releases" / f"{entry_tag}.html").is_file():
        more.append(f'<a href="releases/{entry_tag}.html">Release notes for '
                    f'{esc(entry["tool"])} {esc(entry["version"])}</a>')
    release_link = f'<p class="entry-more">{" · ".join(more)}</p>'
    names = ", ".join(t["name"] for t in tools[:-1]) + f" and {tools[-1]['name']}"
    description = (f"Études is a set of file tools for macOS: {names}. "
                   "Each tool installs and runs on its own.")
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Études: careful file tools for macOS</title>
<meta name="description" content="{esc_attr(description)}">
<meta property="og:title" content="Études">
<meta property="og:description" content="{esc_attr(description)}">
<meta name="color-scheme" content="light dark">
{GENERATOR}
<link rel="stylesheet" href="assets/site.css">
<link rel="icon" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><text y='13' font-size='13'>&#9679;</text></svg>">
</head>
<body class="home">
{topbar(root, "home")}
<main id="main">
<section class="hero" aria-labelledby="title">
<h1 id="title">Études is a set of file tools for macOS.</h1>
<p class="sub">Each tool installs and runs on its own.</p>
<ul class="toolrow" aria-label="The tools">{cards}</ul>
</section>

<section class="band" id="install" aria-labelledby="install-title">
<h2 id="install-title">Install</h2>
<p class="intro">Each tool installs on its own with Cargo. Pick one, or all {count}.</p>
{install_block(tools, versions)}
<p class="needs">Needs Rust. Get it from <a href="https://rustup.rs">rustup.rs</a>.</p>
<p class="visually-hidden" id="copy-status" role="status" aria-live="polite"></p>
<div class="after-install">
<div>
<h3 id="check-it-worked">Check it worked</h3>
<p>Each tool prints its name and version.</p>
{copyable(checks, "the version checks")}
</div>
<div>
<h3 id="update">Update</h3>
<p>Run the same line with the new tag. Replace <code>X.Y.Z</code> with the version from the <a href="changelog/">changelog</a>.</p>
<div class="cmd"><pre><code>{html.escape(update, quote=False)}</code></pre></div>
</div>
</div>
</section>

<section class="band" id="whats-new" aria-labelledby="whats-new-title">
<h2 id="whats-new-title">What's new</h2>
{render_entry(entry)}
{release_link}
</section>

<section class="tagline" aria-label="How the tools behave">
<p>{tagline}</p>
</section>

<section class="band" id="tools" aria-labelledby="tools-title">
<h2 id="tools-title">Commands for each tool</h2>
{blocks}
</section>
</main>
<footer class="sitefoot">
<p>Études is open source under the <a href="{BLOB_URL}LICENSE">Apache 2.0 license</a>.</p>
<nav aria-label="Footer">
<a href="docs/">Docs</a>
<a href="docs/security/">Security</a>
<a href="demo/">Demos</a>
<a href="changelog/">Changelog</a>
<a href="{REPO_URL}">GitHub</a>
</nav>
</footer>
<script>{HOME_SCRIPT}</script>
</body>
</html>
"""


# --- pages ------------------------------------------------------------------

def summary(text, limit=200):
    """A paragraph cut to whole sentences, or whole words, within LIMIT."""
    if len(text) <= limit:
        return text
    cut = text[:limit]
    end = max(cut.rfind(". "), cut.rfind("? "), cut.rfind("! "))
    if end > 0:
        return cut[:end + 1]
    return cut.rsplit(" ", 1)[0] + "…"


def build_doc(page, pages, linker):
    source = page["source"]
    text = (ROOT / source).read_text(encoding="utf-8")
    conv = Converter(source, page["dir"], linker)
    body = conv.convert(text)
    if not conv.title:
        raise BuildError(f"{source}: no top-level heading to use as the page title")
    if not conv.description:
        raise BuildError(f"{source}: no paragraph to use as the page description")
    foot = (f'<p class="pagefoot">This page is built from '
            f'<a href="{BLOB_URL}{source}"><code>{esc(source)}</code></a>.</p>')
    return layout(title=conv.title, description=summary(conv.description), page_dir=page["dir"],
                  pages=pages, content=body + "\n" + foot, headings=conv.headings)


def build_index(pages):
    tools = [p for p in pages if p["tool"]]
    cards = "".join(
        f'<a class="card" href="{rel_dir("docs", p["dir"])}"><span class="name">{esc(p["title"])}</span>'
        f'<span class="desc">{code_text(p["description"])}</span></a>'
        for p in tools
    )
    ids = Ids()
    ids.make("docs")
    tools_id = ids.make("Tools")
    sections, order = {}, []
    for p in pages:
        if p["tool"]:
            continue
        if p["group"] not in sections:
            sections[p["group"]] = []
            order.append(p["group"])
        sections[p["group"]].append(f'<li><a href="{rel_dir("docs", p["dir"])}">{esc(p["title"])}</a></li>')
    group_ids = {g: ids.make(g) for g in order}
    shared = "".join(
        f'<h2 id="{group_ids[g]}"><a class="anchor" href="#{group_ids[g]}">{esc(g)}</a></h2>'
        f'<ul class="pagelist">{"".join(sections[g])}</ul>'
        for g in order
    )
    content = (
        '<h1 id="docs">Docs</h1>\n'
        '<p class="lead">Études is a set of careful file tools for macOS. Each tool installs '
        "and runs on its own. Every move is checked first, and every failure says why.</p>\n"
        f'<h2 id="{tools_id}"><a class="anchor" href="#{tools_id}">Tools</a></h2>\n'
        f'<div class="cards">{cards}</div>\n{shared}'
    )
    headings = [(2, tools_id, "Tools")] + [(2, group_ids[g], esc(g)) for g in order]
    return layout(title="Docs", description="How to use the Études tools, and the rules they share.",
                  page_dir="docs", pages=pages, content=content, headings=headings)


# --- changelog --------------------------------------------------------------

# /changelog/ lists every entry, newest first, then the release pages that have
# no entry. /changelog/<tool>-<version>/ is one entry in the release format:
# the film on top when the entry names one, then the entry itself. The entries
# are written by scripts/release-runs.py from real runs; check-site.py checks
# them against their raw logs.

def entry_slug(entry):
    return f"{entry['tool']}-{entry['version']}"


RELEASE_PAGE = re.compile(r"^([a-z]+)-v(\d+)\.(\d+)\.(\d+)\.html$")


def release_pages():
    """[(tool, version, file name)] for each page in site/releases/, newest version first."""
    found = []
    for path in (SITE / "releases").glob("*.html"):
        m = RELEASE_PAGE.match(path.name)
        if m:
            found.append((m.group(1), f"{m.group(2)}.{m.group(3)}.{m.group(4)}", path.name))
    return sorted(found, key=lambda r: (tuple(int(n) for n in r[1].split(".")), r[0]), reverse=True)


def plain_parts(parts):
    return "".join(part.get("code", part.get("text", "")) for part in parts)


def changelog_nav(entries):
    return [("Changelog", [("changelog", "All releases")]
             + [(f"changelog/{entry_slug(e)}", f"{e['tool']} {e['version']}") for e in entries])]


def film(entry, root):
    """The release film, when the entry names one. Nothing at all until then."""
    media = entry.get("film")
    if not media:
        return ""
    poster = f' poster="{root}{esc_attr(media["still"])}"' if media.get("still") else ""
    return (f'<figure class="film"><video controls playsinline preload="metadata"{poster} '
            f'src="{root}{esc_attr(media["mov"])}"></video>'
            f'<figcaption>{esc(media.get("caption", ""))}</figcaption></figure>')


def build_entry_page(entry, pages, entries):
    slug = entry_slug(entry)
    page_dir = f"changelog/{slug}"
    root = rel_dir(page_dir, "")
    links = [f'<a href="../">All releases</a>']
    tag = f'{entry["tool"]}-v{entry["version"]}'
    if (SITE / "releases" / f"{tag}.html").is_file():
        links.append(f'<a href="{root}releases/{tag}.html">Release notes for '
                     f'{esc(entry["tool"])} {esc(entry["version"])}</a>')
    content = (f'{film(entry, root)}\n{render_entry(entry, level=1)}\n'
               f'<p class="entry-more">{" · ".join(links)}</p>')
    return layout(title=f'{entry["tool"]} {entry["version"]}', description=summary(plain_parts(entry["lead"])),
                  page_dir=page_dir, pages=pages, content=content, headings=[], current="changelog",
                  first=changelog_nav(entries), body_class="changelog")


def build_changelog_index(pages, entries):
    ids = Ids()
    ids.make("changelog")
    items, headings = [], []
    for entry in entries:
        slug = entry_slug(entry)
        hid = ids.make(slug)
        name = f'{entry["tool"]} {entry["version"]}'
        items.append(
            f'<section class="cl-item" aria-labelledby="{hid}">'
            f'<p class="entry-meta"><span class="entry-ver">{esc(name)}</span>'
            f'<span class="pill">{esc(entry["label"])}</span>{release_meta(entry)}</p>'
            f'<h2 id="{hid}"><a href="{slug}/">{esc(entry["headline"])}</a></h2>'
            f'<p class="entry-lead">{rich(entry["lead"])}</p></section>')
        headings.append((2, hid, esc(name)))
    with_entry = {entry_slug(e) for e in entries}
    others = [(tool, version, name) for tool, version, name in release_pages()
              if f"{tool}-{version}" not in with_entry]
    rest = ""
    if others:
        rid = ids.make("Release notes")
        rest = (f'<h2 id="{rid}"><a class="anchor" href="#{rid}">Release notes</a></h2>'
                f'<p>Releases with notes and no entry yet.</p><ul class="pagelist">'
                + "".join(f'<li><a href="../releases/{esc_attr(name)}">{esc(tool)} {esc(version)}</a></li>'
                          for tool, version, name in others)
                + "</ul>")
        headings.append((2, rid, "Release notes"))
    content = ('<h1 id="changelog">Changelog</h1>\n'
               '<p class="lead">What changed in each release, shown with real runs of the build before '
               f'and the build after. Every change is also in <a href="{CHANGELOG_FILE_URL}">CHANGELOG.md</a>.</p>\n'
               + "\n".join(items) + "\n" + rest)
    return layout(title="Changelog", description="What changed in each Études release, with real before and after runs.",
                  page_dir="changelog", pages=pages, content=content, headings=headings,
                  current="changelog", first=changelog_nav(entries), body_class="changelog")


def prepare_out(out):
    if out.exists():
        if not out.is_dir():
            raise SystemExit(f"FAIL {out} exists and is not a folder; nothing was written")
        index = out / "docs" / "index.html"
        ours = index.is_file() and GENERATOR in index.read_text(encoding="utf-8", errors="replace")
        if any(out.iterdir()) and not ours:
            raise SystemExit(f"FAIL {out} is not empty and was not made by this script; "
                             "nothing was written. Choose an empty or new folder with --out.")
        shutil.rmtree(out)
    out.mkdir(parents=True)


def build(out):
    pages = public_pages()
    linker = Linker(pages)
    rendered = {page["dir"]: build_doc(page, pages, linker) for page in pages}
    rendered["docs"] = build_index(pages)
    rendered[""] = build_home(pages)
    entries = changelog_entries()
    rendered["changelog"] = build_changelog_index(pages, entries)
    for entry in entries:
        rendered[f"changelog/{entry_slug(entry)}"] = build_entry_page(entry, pages, entries)
    prepare_out(out)
    plain_ignore = shutil.ignore_patterns(*BUILD_ONLY)

    def ignore(folder, names):
        skipped = set(plain_ignore(folder, names))
        if Path(folder) == SITE:
            skipped |= {n for n in names if n in BUILD_ONLY_DIRS}
        return skipped

    shutil.copytree(SITE, out, dirs_exist_ok=True, ignore=ignore)
    for page_dir, page_html in rendered.items():
        target = out / page_dir / "index.html" if page_dir else out / "index.html"
        if target.exists():
            raise BuildError(f"{page_dir}/index.html is both a file in site/ and a built page")
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(page_html, encoding="utf-8")
    return pages


def main(argv):
    out = ROOT / "_site"
    if len(argv) == 3 and argv[1] == "--out":
        out = Path(argv[2]).resolve()
    elif len(argv) != 1:
        print(__doc__.strip().split("\n\n")[1], file=sys.stderr)
        return 2
    try:
        pages = build(out)
    except BuildError as error:
        print(f"FAIL the site did not build: {error}", file=sys.stderr)
        return 1
    except SystemExit as error:
        print(error, file=sys.stderr)
        return 2
    print(f"ok   built the home page, {len(pages)} docs pages, the docs index and the changelog into {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
