#!/usr/bin/env python3
"""Build the Études site from site/ and the public docs.

    scripts/build-site.py              build into _site/
    scripts/build-site.py --out DIR    build into DIR

Everything under site/ is copied as it is (except the two data files the build
reads), so the home page, the release pages, the GIFs and transcripts.json keep
their URLs and stay byte-identical to what check-claims.sh and
check-transcripts-reproduce.sh check. Then each public doc becomes
/docs/<slug>/index.html in the docs layout.

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
import html.entities
import json
import posixpath
import re
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SITE = ROOT / "site"
REPO_URL = "https://github.com/risingmoonscholar/etudes"
BLOB_URL = REPO_URL + "/blob/main/"
TREE_URL = REPO_URL + "/tree/main/"
# The changelog pages are a later change; until then the top bar points at the
# changelog file itself.
CHANGELOG_URL = BLOB_URL + "CHANGELOG.md"
# Every built docs page carries this, so a rebuild can tell its own output
# folder from someone else's before it deletes anything.
GENERATOR = '<meta name="generator" content="scripts/build-site.py">'
# Read by the build, not by any page.
BUILD_ONLY = ("tools.json", "docs.json", ".DS_Store")
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
        for key in ("name", "description", "docs"):
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

def sidebar(pages, current_dir):
    def link(href_dir, label, mono=False):
        href = rel_dir(current_dir, href_dir)
        cur = ' aria-current="page"' if href_dir == current_dir else ""
        text = f"<code>{esc(label)}</code>" if mono else esc(label)
        return f'<li><a href="{esc_attr(href)}"{cur}>{text}</a></li>'

    groups = [("Get started", [link("docs", "Overview")])]
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


def layout(*, title, description, page_dir, pages, content, headings):
    root = rel_dir(page_dir, "")
    nav = sidebar(pages, page_dir)
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
<body>
<header class="topbar">
<a class="brand" href="{root}">Études</a>
<nav class="topnav" aria-label="Site">
<a href="{root}docs/" aria-current="true">Docs</a>
<a href="{CHANGELOG_URL}">Changelog</a>
<a href="{REPO_URL}">GitHub</a>
</nav>
</header>
<div class="layout">
<details class="menu"><summary>Menu</summary><nav aria-label="Docs menu">
{nav}
</nav></details>
<aside class="sidebar" aria-label="Docs">
<nav>
{nav}
</nav>
</aside>
<main class="content">
{content}
</main>
{right}
</div>
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
        f'<span class="desc">{esc(p["description"])}</span></a>'
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
    prepare_out(out)
    shutil.copytree(SITE, out, dirs_exist_ok=True, ignore=shutil.ignore_patterns(*BUILD_ONLY))
    for page_dir, page_html in rendered.items():
        target = out / page_dir / "index.html"
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
    print(f"ok   built {len(pages)} docs pages and the docs index into {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
