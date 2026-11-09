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
  - the published demo page or transcripts.json differs from site/, so the
    panels check-transcripts-reproduce.sh proves are the panels published;
  - the home page's install block names a version other than the one in that
    tool's Cargo.toml (the source check-release.py holds the README pins to),
    or leaves a tool out;
  - captured output on the home page differs from site/transcripts.json, or a
    command it explains is not a usage line of that tool's captured help;
  - the home page is not what the build renders from its data;
  - fonts are published without their license.

Standard library only. Exit 0 checked and clean, 1 a failure, 2 could not run.
"""
import html
import importlib.util
import json
import posixpath
import re
import subprocess
import sys
import tempfile
import tomllib
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
REPO_URL = "https://github.com/risingmoonscholar/etudes"
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


def load_build():
    spec = importlib.util.spec_from_file_location("build_site", ROOT / "scripts/build-site.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


INSTALL = re.compile(r"cargo install --locked --git (\S+) --tag ([a-z][a-z0-9-]*?)-v(\d+\.\d+\.\d+) ([A-Za-z0-9_-]+)")


def manifest_versions(tools):
    """{tool: (package, version)} read straight from each Cargo.toml, as check-release.py reads them."""
    found = {}
    for tool in tools:
        package = tomllib.loads((ROOT / tool["crate"] / "Cargo.toml").read_text())["package"]
        found[tool["name"]] = (package["name"], package["version"])
    return found


def check_entry(entry, tag_exists):
    """Problems with a release entry's footer. TAG_EXISTS: is <tool>-v<version> a tag here?"""
    problems = []
    footer = entry.get("footer", "")
    for side in ("before", "after"):
        commit = entry.get("builds", {}).get(side, {}).get("commit")
        if not commit or commit not in footer:
            problems.append(f"the What's new footer does not name the {side} build's commit {commit!r}")
    tag = f"{entry['tool']}-v{entry['version']}"
    says_untagged = "not tagged yet" in footer
    if entry.get("builds", {}).get("after", {}).get("tagged") is False:
        if not says_untagged:
            problems.append("the What's new entry's after build is not tagged, and its footer does not say so")
        if tag_exists:
            problems.append(f"the What's new footer says the {entry['version']} build is not tagged yet, "
                            f"but the tag {tag} exists. Run the entry again on the tag and update "
                            f"site/changelog/{entry['tool']}-{entry['version']}.json")
    elif says_untagged:
        problems.append("the What's new footer says not tagged yet, but the entry records a tagged build")
    return problems


def check_home(out):
    """The home page: its install lines, its captured output, and that it is what the build renders."""
    problems, notes = [], []
    home = out / "index.html"
    if not home.is_file():
        return ["index.html (the home page) was not built"], notes
    text = home.read_text(encoding="utf-8")
    tools = json.loads((ROOT / "site/tools.json").read_text())["tools"]

    # Install versions: every install line names the version in that tool's
    # Cargo.toml, the same source check-release.py holds the README pins to.
    expected = manifest_versions(tools)
    lines = INSTALL.findall(html.unescape(text))
    named = set()
    for repo, tool, version, package in lines:
        named.add(tool)
        if repo != REPO_URL:
            problems.append(f"the install line for {tool} installs from {repo}, not {REPO_URL}")
        if tool not in expected:
            problems.append(f"the install block names {tool}-v{version}, and {tool} is not in site/tools.json")
            continue
        want_package, want_version = expected[tool]
        if version != want_version:
            problems.append(f"the install block names {tool} {version}; "
                            f"{tools[[t['name'] for t in tools].index(tool)]['crate']}/Cargo.toml declares {want_version}")
        if package != want_package:
            problems.append(f"the install block installs {package} for {tool}; its crate is {want_package}")
    for tool in expected:
        if tool not in named:
            problems.append(f"the install block has no install line for {tool}")
    notes.append(f"{len(lines)} install lines name the versions in the tools' Cargo.toml")

    # Captured output on the home page is the captured output in transcripts.json.
    captured = {t["label"]: t for t in json.loads((ROOT / "site/transcripts.json").read_text())["transcripts"]}
    shown = re.findall(r'<pre class="term out" data-transcript="([^"]+)"><code>(.*?)</code></pre>', text, re.S)
    for label, body in shown:
        if label not in captured:
            problems.append(f"the home page shows a run {label!r} that site/transcripts.json does not hold")
        elif html.unescape(body) != captured[label]["output"].rstrip():
            problems.append(f"the home page's {label!r} output differs from site/transcripts.json")
    # And each command it explains is a usage line of that tool's captured help.
    for tool in tools:
        help_text = captured.get(f"{tool['name']}-help", {}).get("output", "")
        for command in tool.get("commands", []):
            if not re.search(rf"^\s*{re.escape(command['usage'])}(\s|$)", help_text, re.M):
                problems.append(f"site/tools.json: {tool['name']}: {command['usage']!r} is not a usage line "
                                f"in the captured {tool['name']} help")
    notes.append(f"{len(shown)} captured runs on the home page match site/transcripts.json; "
                 "every command shown is in its tool's help")

    # The What's new footer names the builds the runs came from, and says
    # "not tagged yet" only while that stays true.
    try:
        entry = load_build().latest_entry()
    except Exception as error:
        problems.append(f"the What's new entry could not be read: {error}")
    else:
        tag = f"{entry['tool']}-v{entry['version']}"
        tags = subprocess.run(["git", "tag", "--list", tag], cwd=ROOT, capture_output=True, text=True)
        problems += check_entry(entry, tags.returncode == 0 and tags.stdout.strip() == tag)
    notes.append("the What's new footer names its builds, and says not tagged only while it is not")

    # The page is exactly what the build renders from the data today, so a
    # hand edit, or data that changed after the build, is caught.
    try:
        build = load_build()
        rendered = build.build_home(build.public_pages())
    except Exception as error:  # the build's own error says what is wrong
        problems.append(f"the home page could not be rendered for comparison: {error}")
    else:
        if rendered != text:
            problems.append("the published index.html is not what scripts/build-site.py renders "
                            "from site/tools.json, site/changelog/ and the Cargo.toml versions")
    notes.append("the home page is what the build renders from its data")
    return problems, notes


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
        main = re.search(r'<main class="content"[^>]*>(.*)</main>', text, re.S)
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
    for name in ("demo/index.html", "transcripts.json"):
        built = out / name
        if not built.is_file() or built.read_bytes() != (ROOT / "site" / name).read_bytes():
            problems.append(f"the published {name} is not byte-identical to site/{name}")
    notes.append("the published demo page and transcripts.json are byte-identical to site/")

    home_problems, home_notes = check_home(out)
    problems += home_problems
    notes += home_notes

    for name in ("tools.json", "docs.json", "changelog"):
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
