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
  - the home page is not what the build renders from its data, or its What's
    new is not the newest entry in site/changelog/;
  - a changelog log or record names a machine's own folder (/var/folders,
    /Users, /tmp) instead of <scratch>;
  - a changelog entry has no fixture or no raw logs, a log differs from the
    sha256 its record.json holds, or the entry differs from what
    scripts/release-runs.py derives from the record, the logs and the fixture
    (an output line, exit code, command, result file, build or runs note edited
    by hand); or the fixture's setup, commands or result globs changed after
    the runs;
  - the tag of the version being released (the tool's Cargo.toml version)
    is lightweight, not made with git tag -a;
  - with no <tool>-v<version> tag, an entry's after build is not an ancestor
    of HEAD; with the tag, the tag names a commit whose tool source differs
    from the after build's; or the entry's before tag does not name its
    before commit;
  - a changelog page is not what the build renders, or the changelog's data,
    fixtures or logs are published;
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


def load_script(name, path):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def load_build():
    return load_script("build_site", "scripts/build-site.py")


def load_runs():
    return load_script("release_runs", "scripts/release-runs.py")


def git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True)


def tag_rev(name):
    """The commit tag NAME points at, or None when this clone has no such tag."""
    done = git("rev-parse", "--verify", "--quiet", f"refs/tags/{name}^{{commit}}")
    return done.stdout.strip() if done.returncode == 0 else None


def shallow():
    return git("rev-parse", "--is-shallow-repository").stdout.strip() == "true"


INSTALL = re.compile(r"cargo install --locked --git (\S+) --tag ([a-z][a-z0-9-]*?)-v(\d+\.\d+\.\d+) ([A-Za-z0-9_-]+)")


def manifest_versions(tools):
    """{tool: (package, version)} read straight from each Cargo.toml, as check-release.py reads them."""
    found = {}
    for tool in tools:
        package = tomllib.loads((ROOT / tool["crate"] / "Cargo.toml").read_text())["package"]
        found[tool["name"]] = (package["name"], package["version"])
    return found


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

    # What's new shows the newest entry: a release candidate first, then the
    # latest tag date, then the highest version. The order is worked out here,
    # from the tag state git gives, not taken from the build's own sort.
    try:
        build = load_build()
        entries = [json.loads(p.read_text(encoding="utf-8"))
                   for p in sorted((ROOT / "site/changelog").glob("*.json"))]

        def order(e):
            state = build.release_state(e)
            return (0 if state["tagged"] else 1, state.get("date") or "",
                    tuple(int(n) for n in e["version"].split(".")))
        newest = max(entries, key=order)
        article = build.render_entry(newest)
    except Exception as error:
        problems.append(f"the newest changelog entry could not be read: {error}")
    else:
        section = re.search(r'<section class="band" id="whats-new".*?</section>', text, re.S)
        if not section or article not in section.group(0):
            problems.append(f"What's new does not show the newest entry, "
                            f"site/changelog/{newest['tool']}-{newest['version']}.json")
    notes.append("What's new shows the newest changelog entry")

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


def check_builds(entry):
    """Problems with the commits an entry names, as git sees them here.

    The before build's tag must name its commit. The release's own tag state
    is not stored in the entry; scripts/build-site.py reads it from git. Here:
    with no tag, the after build is a candidate and must be an ancestor of
    HEAD; with a tag, the tag must name that commit or one with the same tool
    source (build-site.py's release_state says which, or stops).
    """
    problems = []
    name = f"site/changelog/{entry['tool']}-{entry['version']}.json"
    builds = entry.get("builds", {})
    before, after = builds.get("before", {}), builds.get("after", {})
    if before.get("tag"):
        before_tag = before["tag"]
        rev = tag_rev(before_tag)
        if rev and rev != before.get("rev"):
            problems.append(f"{name}: the before build is {before_tag}, which names {rev[:7]}, "
                            f"not {before.get('commit')}")
        elif not rev and not shallow():
            problems.append(f"{name}: the before build is {before_tag}, and this repository has no such tag")
    try:
        state = load_build().release_state(entry)
    except Exception as error:  # the build's own error says what to run
        problems.append(f"{name}: {error}")
        return problems
    # The release being made now (the version in the tool's Cargo.toml) must be
    # tagged with `git tag -a`: its date on the site is the tagger date. Older
    # lightweight tags stay as they are and show "Tagged" with no date.
    if state["tagged"] and not state.get("annotated"):
        manifest = ROOT / "crates" / f"{entry['tool']}-cli" / "Cargo.toml"
        current = tomllib.loads(manifest.read_text())["package"]["version"] if manifest.is_file() else None
        if entry["version"] == current:
            tag, commit = state["tag"], state["tag_commit"]
            problems.append(
                f"{name}: {tag} is a lightweight tag; the release needs an annotated tag "
                f"(git tag -a), so its date comes from the tag. Recreate it on the same commit: "
                f"git tag -d {tag} && git tag -a {tag} {commit} -m '{entry['tool']} {entry['version']}'. "
                f"If it was pushed, also run git push origin :refs/tags/{tag} before "
                f"git push origin {tag}. Same commit, type only (see RELEASING.md)")
    if not state["tagged"]:
        known = git("cat-file", "-e", f"{after.get('rev')}^{{commit}}").returncode == 0
        if not known:
            if not shallow():
                problems.append(f"{name}: the after build's commit {after.get('commit')} is not in this repository")
        elif git("merge-base", "--is-ancestor", after["rev"], "HEAD").returncode != 0:
            problems.append(f"{name}: the after build's commit {after.get('commit')} is not an ancestor of HEAD, "
                            "so the release candidate it names is not this branch's code")
    return problems


def check_entries(folder=None):
    """Every entry in FOLDER (site/changelog/) against its fixture, its raw logs and git."""
    problems, notes = [], []
    folder = Path(folder or ROOT / "site/changelog")
    runs = load_runs()
    paths = sorted(folder.glob("*.json"))
    for fixture in sorted(folder.glob("*.fixture")):
        if not fixture.with_suffix(".json").is_file():
            problems.append(f"site/changelog/{fixture.name} has no entry; run scripts/release-runs.py "
                            f"or remove the fixture")
    for path in paths:
        name = f"site/changelog/{path.name}"
        try:
            entry = json.loads(path.read_text(encoding="utf-8"))
            tool, version = entry["tool"], entry["version"]
        except (OSError, json.JSONDecodeError, KeyError) as error:
            problems.append(f"{name} cannot be read: {error}")
            continue
        rerun = (f"Run scripts/release-runs.py {tool} {version} --before <previous tag> "
                 f"--after <this release> again")
        fixture_file = folder / runs.fixture_path(tool, version).name
        if not fixture_file.is_file():
            problems.append(f"{name} has no fixture, site/changelog/{fixture_file.name}")
            continue
        logs = folder / "logs" / path.stem
        if not (logs / "record.json").is_file():
            problems.append(f"{name} has no raw logs: site/changelog/logs/{path.stem}/record.json is missing")
            continue
        try:
            fixture = runs.load_fixture(fixture_file)
            record = json.loads((logs / "record.json").read_text(encoding="utf-8"))
        except (runs.RunError, OSError, json.JSONDecodeError) as error:
            problems.append(f"{name}: {error}")
            continue
        bad_logs = []
        for run in [r for s in runs.SIDES for r in record["sides"][s]["runs"]] + [record["next"]]:
            log = logs / run["log"]
            if not log.is_file() or runs.sha256_file(log) != run["sha256"]:
                bad_logs.append(run["log"])
        for log in sorted(logs.iterdir()):
            if runs.MACHINE_PATH.search(log.read_text(encoding="utf-8", errors="replace")):
                problems.append(f"site/changelog/logs/{path.stem}/{log.name} holds a machine's own folder "
                                f"path; the logs write the scratch folder as {runs.SCRATCH}. {rerun}")
        if bad_logs:
            problems.append(f"{name}: the raw log(s) {', '.join(bad_logs)} differ from the sha256 in "
                            f"site/changelog/logs/{path.stem}/record.json. {rerun}")
            continue
        if record.get("runs_sha256") != runs.runs_digest(fixture):
            problems.append(f"{name}: site/changelog/{fixture_file.name} changed its setup, commands or "
                            f"result globs after the runs. {rerun}")
        try:
            derived = runs.entry_from_record(record, fixture, runs.hand_fields(entry),
                                             lambda n: (logs / n).read_text(encoding="utf-8"))
        except runs.RunError as error:
            problems.append(f"{name}: {error}")
            continue
        if derived != entry:
            keys = sorted(k for k in set(derived) | set(entry) if derived.get(k) != entry.get(k))
            problems.append(f"{name} differs from its raw logs in {', '.join(keys)}: an output line, exit code "
                            f"or build was edited by hand, or the fixture's cuts changed. Run the runs "
                            f"again, or put the field back as scripts/release-runs.py writes it")
        problems += check_builds(entry)
    notes.append(f"{len(paths)} changelog entries match their fixtures, raw logs and builds")
    if shallow():
        notes.append("this clone is shallow, so it may not hold every tag and commit the entries name; "
                     "the tag and ancestor checks skipped what it lacks (pages.yml fetches all of it)")

    return problems, notes


def check_changelog(out):
    """The entries, and every changelog page as the build renders it."""
    problems, notes = check_entries()
    build = load_build()
    # The pages, as the build renders them today; and none of the data published.
    try:
        pages = build.public_pages()
        entries = build.changelog_entries()
        want = {"changelog/index.html": build.build_changelog_index(pages, entries)}
        for entry in entries:
            want[f"changelog/{build.entry_slug(entry)}/index.html"] = build.build_entry_page(entry, pages, entries)
    except Exception as error:  # the build's own error says what is wrong
        problems.append(f"the changelog pages could not be rendered for comparison: {error}")
        return problems, notes
    for rel, page_html in want.items():
        page = out / rel
        if not page.is_file():
            problems.append(f"{rel} was not built")
        elif page.read_text(encoding="utf-8") != page_html:
            problems.append(f"the published {rel} is not what scripts/build-site.py renders from site/changelog/")
    if (out / "changelog").is_dir():
        for path in sorted((out / "changelog").rglob("*")):
            rel = path.relative_to(out).as_posix()
            if path.is_file() and rel not in want:
                problems.append(f"{rel} is published; the changelog publishes its pages only, "
                                "not its data, fixtures or logs")
    notes.append(f"{len(want)} changelog pages are what the build renders, and no changelog data is published")
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

    log_problems, log_notes = check_changelog(out)
    problems += log_problems
    notes += log_notes

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
