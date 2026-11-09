#!/usr/bin/env python3
"""Does the site build, and does every link on it land somewhere?

    scripts/check-site.py              build into a temporary folder and check it
    scripts/check-site.py --dir DIR    check a build that already exists (pages.yml)

Fails when:
  - scripts/build-site.py does not build;
  - a public doc in site/tools.json or site/docs.json has no page, or docs/
    holds a page that is not on that list;
  - a page links to a file that is not in the build, or to an anchor that is
    not on the page it names (links that leave the site are not fetched);
  - a stylesheet's url() names a file that is not in the build (a font, say);
  - a page has two elements with the same id, or an empty id;
  - a docs page shows raw Markdown: **, ```, |--- or ]( outside code;
  - the published home page or transcripts.json differs from site/, so the
    panels check-transcripts-reproduce.sh proves are the panels published;
  - fonts are published without their license.

Standard library only. Exit 0 checked and clean, 1 a failure, 2 could not run.
"""
import json
import posixpath
import re
import subprocess
import sys
import tempfile
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
LEAKS = ("**", "```", "|---", "](")


class Page(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.ids = set()
        self.id_list = []
        self.links = []

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if "id" in attrs:
            self.id_list.append(attrs["id"] or "")
        if attrs.get("id"):
            self.ids.add(attrs["id"])
        if tag == "a" and attrs.get("name"):
            self.ids.add(attrs["name"])
        for key in ("href", "src"):
            if attrs.get(key) is not None and tag in ("a", "link", "img", "script", "source"):
                self.links.append((tag, attrs[key]))


def parse(path, cache):
    if path not in cache:
        page = Page()
        page.feed(path.read_text(encoding="utf-8", errors="replace"))
        cache[path] = page
    return cache[path]


def target_file(out, page_rel, url):
    """The file in OUT that URL on PAGE_REL names, or None when it leaves the site."""
    parts = urlsplit(url)
    if parts.scheme or parts.netloc:
        return None, None
    path = unquote(parts.path)
    if not path:
        return out / page_rel, parts.fragment
    if path.startswith("/"):
        # Pages serve the site under /etudes/; an absolute path would break there.
        return "ABSOLUTE", parts.fragment
    joined = posixpath.normpath(posixpath.join(posixpath.dirname(page_rel), path))
    if joined.startswith(".."):
        return "OUTSIDE", parts.fragment
    target = out / joined
    if path.endswith("/") or target.is_dir():
        target = target / "index.html"
    return target, parts.fragment


def check(out):
    problems = []
    notes = []

    # The public list, from the same data the build reads.
    tools = json.loads((ROOT / "site/tools.json").read_text())["tools"]
    sections = json.loads((ROOT / "site/docs.json").read_text())["sections"]
    sources = [t["docs"] for t in tools] + [p["source"] for s in sections for p in s["pages"]]
    slugs = {Path(s).stem.lower() for s in sources}
    for slug in sorted(slugs):
        if not (out / "docs" / slug / "index.html").is_file():
            problems.append(f"docs/{slug}/ was not built, though it is on the public list")
    if not (out / "docs/index.html").is_file():
        problems.append("docs/index.html was not built")
    for child in sorted((out / "docs").iterdir()) if (out / "docs").is_dir() else []:
        if child.is_dir() and child.name not in slugs:
            problems.append(f"docs/{child.name}/ is published but is not on the public list")
    notes.append(f"{len(slugs)} public docs pages, each built")

    # Links and anchors, on every page in the build.
    cache = {}
    pages = sorted(out.rglob("*.html"))
    checked = 0
    for page_path in pages:
        page_rel = page_path.relative_to(out).as_posix()
        page = parse(page_path, cache)
        for tag, url in page.links:
            if url.startswith(("data:", "mailto:")):
                continue
            target, frag = target_file(out, page_rel, url)
            if target is None:
                continue
            checked += 1
            if target == "ABSOLUTE":
                problems.append(f"{page_rel}: {url} is an absolute path; it breaks under the Pages subpath")
                continue
            if target == "OUTSIDE":
                problems.append(f"{page_rel}: {url} points outside the site")
                continue
            if not target.is_file():
                problems.append(f"{page_rel}: {url} names {target.relative_to(out).as_posix()}, which is not in the build")
                continue
            if frag:
                if target.suffix != ".html":
                    problems.append(f"{page_rel}: {url} has an anchor on a file that is not a page")
                elif frag not in parse(target, cache).ids:
                    problems.append(f"{page_rel}: {url} names #{frag}, which is not on {target.relative_to(out).as_posix()}")
    notes.append(f"{checked} internal links and anchors across {len(pages)} pages resolve")

    # Anchors have to be unique and named, or a link lands on the wrong place.
    for page_path in pages:
        page_rel = page_path.relative_to(out).as_posix()
        ids = parse(page_path, cache).id_list
        if "" in ids:
            problems.append(f"{page_rel}: an element has an empty id")
        dupes = sorted({i for i in ids if i and ids.count(i) > 1})
        if dupes:
            problems.append(f"{page_rel}: duplicate id(s): {', '.join(dupes)}")
    notes.append("every id on every page is unique and not empty")

    # Files a stylesheet loads: fonts, images.
    css_refs = 0
    for css in sorted(out.rglob("*.css")):
        css_rel = css.relative_to(out).as_posix()
        for url in re.findall(r"url\(\s*['\"]?([^'\")]+?)['\"]?\s*\)", css.read_text(encoding="utf-8")):
            if url.startswith(("data:", "#")) or re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", url):
                continue
            css_refs += 1
            target, _ = target_file(out, css_rel, url)
            if target in ("ABSOLUTE", "OUTSIDE") or not Path(target).is_file():
                problems.append(f"{css_rel}: url({url}) is not in the build")
    notes.append(f"{css_refs} files the stylesheets load are in the build")

    # Raw Markdown on a built docs page, outside code.
    for page_path in sorted((out / "docs").rglob("index.html")) if (out / "docs").is_dir() else []:
        text = page_path.read_text(encoding="utf-8")
        main = re.search(r'<main class="content">(.*)</main>', text, re.S)
        if not main:
            problems.append(f"{page_path.relative_to(out).as_posix()}: no content area")
            continue
        body = re.sub(r"<pre>.*?</pre>", "", main.group(1), flags=re.S)
        body = re.sub(r"<code[^>]*>.*?</code>", "", body, flags=re.S)
        for token in LEAKS:
            at = body.find(token)
            if at >= 0:
                context = re.sub(r"\s+", " ", body[max(0, at - 40):at + 30])
                problems.append(f"{page_path.relative_to(out).as_posix()}: raw Markdown {token!r} on the page: ...{context}...")
    notes.append("no raw Markdown outside code on any docs page")

    # The published panels are the checked panels.
    for name in ("index.html", "transcripts.json"):
        built = out / name
        if not built.is_file() or built.read_bytes() != (ROOT / "site" / name).read_bytes():
            problems.append(f"the published {name} is not byte-identical to site/{name}")
    notes.append("the published home page and transcripts.json are byte-identical to site/")

    for name in ("tools.json", "docs.json"):
        if (out / name).exists():
            problems.append(f"{name} is published; the build reads it and no page needs it")

    fonts = list(out.rglob("*.ttf")) + list(out.rglob("*.woff2"))
    if fonts and not all((f.parent / "OFL.txt").is_file() for f in fonts):
        problems.append("fonts are published without OFL.txt beside them")

    return problems, notes


def main(argv):
    if len(argv) == 3 and argv[1] == "--dir":
        out = Path(argv[2]).resolve()
        if not out.is_dir():
            print(f"FAIL no build at {out}. Run scripts/build-site.py --out {argv[2]} first.")
            return 2
        problems, notes = check(out)
    elif len(argv) == 1:
        with tempfile.TemporaryDirectory(prefix="etudes-site-") as tmp:
            out = Path(tmp) / "site"
            built = subprocess.run([sys.executable, str(ROOT / "scripts/build-site.py"), "--out", str(out)],
                                   capture_output=True, text=True)
            if built.returncode != 0:
                print("FAIL the site does not build:")
                print((built.stdout + built.stderr).rstrip())
                return 1
            problems, notes = check(out)
    else:
        print(__doc__.strip().split("\n\n")[1], file=sys.stderr)
        return 2
    for problem in problems:
        print(f"FAIL {problem}")
    if problems:
        print(f"{len(problems)} problem(s). Fix the doc, the link or site/*.json, then run this again.")
        return 1
    for note in notes:
        print(f"ok   {note}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
