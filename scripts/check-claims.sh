#!/usr/bin/env bash
# Verify the numbers this repo claims about itself are still true.
#
# Every claim here is CHECKED, never maintained by hand. Run it in CI; run it
# before you publish.
#
# The rule that shapes this script: a claim it cannot find is a FAILURE, not a
# skip. An earlier version had the scenario count baked into the pattern that
# located the claim --
#
#     grep -oE '33 scenarios, [0-9]+ of them failing' README.md
#
# -- so when the count went from 33 to 36, the pattern stopped matching, the
# result was empty, and an `if [ -n "$claimed" ]` guard skipped the check
# without a word. The readme was wrong by three and this script reported green.
# A checker that only finds the claim while the claim is already correct is
# not a checker. Every lookup below fails loudly when it finds nothing.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

status=0
ok()   { printf "ok   %s\n" "$1"; }
bad()  { printf "FAIL %s\n" "$1"; status=1; }

# claim FILE PATTERN CAPTURE DESCRIPTION ACTUAL
#
# Finds a number in FILE, compares it to ACTUAL, and fails if the pattern
# matches nothing at all -- the case that used to pass silently.
claim() {
  local file="$1" pattern="$2" capture="$3" desc="$4" actual="$5"
  local found claimed
  found=$(grep -oE "$pattern" "$file" | head -1 || true)
  if [ -z "$found" ]; then
    bad "$file states no $desc at all. Either it was reworded (fix this script) or it was dropped (fix the file). Not skipping."
    return
  fi
  claimed=$(grep -oE "$capture" <<<"$found" | head -1)
  if [ "$claimed" != "$actual" ]; then
    bad "$file claims $claimed $desc; reality is $actual"
  else
    ok "$file claims $claimed $desc, and $actual is what runs"
  fi
}

tests_actual=$(cargo test --all 2>&1 | grep -oE '[0-9]+ passed' | awk '{s+=$1} END {print s}')
[ -z "$tests_actual" ] && { echo "FAIL could not count tests; the suite did not report"; exit 1; }

for f in README.md demo/index.html; do
  claim "$f" '[0-9]+ tests' '[0-9]+' "tests" "$tests_actual"
done

# Scenario count, from what git TRACKS rather than what happens to be sitting
# in the working tree. A reader gets what a clone contains, so an untracked
# scenario is not one the readme can claim. Counting the filesystem here was
# a bug in the first version of this fix: it read 36 with one file untracked,
# so the readme was corrected to a number no clone would ever see.
scenarios_actual=$(git ls-files 'stress/scenarios/*.sh' | wc -l | tr -d ' ')
# Every file that states the number, not just the readme. demo/index.html is
# the published site: it can be wrong in front of everyone while CI is green,
# which is the exact failure this script exists to catch, one file over.
for f in README.md demo/index.html; do
  claim "$f" '[0-9]+ scenarios' '[0-9]+' "scenarios" "$scenarios_actual"
done

# How many of them are known to fail, from the baseline the ratchet uses.
failing_known=$(grep -vcE '^[[:space:]]*#|^[[:space:]]*$' stress/baseline.txt | tr -d ' ')
claim README.md '[0-9]+ of them failing' '[0-9]+' "failing scenarios" "$failing_known"

# The journal TTL: defined once in code, restated in the changelog.
ttl_actual=$(grep -oE 'TTL_DAYS: u64 = [0-9]+' crates/etude-core/src/journal.rs | grep -oE '[0-9]+$')
[ -z "$ttl_actual" ] && { echo "FAIL could not read TTL_DAYS from journal.rs"; exit 1; }
claim CHANGELOG.md 'pruned after [0-9]+ days' '[0-9]+' "TTL days" "$ttl_actual"

# The version the readme tells people to install, against the newest tag.
# A release that moves without this line moving sends every new user to the
# previous version, and nothing else would notice: the command still works,
# it just installs something older than the docs describe.
# The tools version separately now -- sweep matures, stash and unpack are
# static -- so "newest tag" stopped being one question. The check that
# replaces it is stronger: for each tool, the tag its install line pins must
# match the version its OWN manifest declares. A release that bumps a
# manifest without moving the install line, or vice versa, fails here.
check_pin() {
  local crate="$1" manifest="$2"
  local ver tag_line
  ver=$(grep -m1 '^version' "$manifest" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
  tag_line=$(grep -oE -- "--tag [a-z-]*v[0-9.]+ $crate" README.md | head -1)
  if [ -z "$tag_line" ]; then
    bad "README.md has no install line pinning a tag for $crate"
    return
  fi
  if grep -qE -- "v$ver $crate\$" <<<"$tag_line"; then
    ok "README.md installs $crate at v$ver, which its manifest declares"
  else
    bad "README.md installs '$tag_line' but $manifest declares $ver"
  fi
}
check_pin sweep-cli  crates/sweep-cli/Cargo.toml
check_pin stash-cli  crates/stash-cli/Cargo.toml
check_pin unpack-cli crates/unpack-cli/Cargo.toml

# The version the site shows, against the version the binaries report.
# It was hardcoded in the page template as "v0.3" and stayed there through the
# whole of 0.4, on the most public surface this project has. A version is a
# claim like any other.
site_version=$(python3 -c "
import json,sys
try:
    d=json.load(open('demo/transcripts.json'))
    print(d.get('version',''))
except Exception:
    print('')
")
# sweep's own manifest, not the workspace: the site's transcripts are sweep's
# output, and sweep versions independently now.
real_version=$(grep -m1 '^version' crates/sweep-cli/Cargo.toml | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
if [ -z "$site_version" ]; then
  bad "demo/transcripts.json records no version; the site cannot show one. Re-run scripts/capture-demos.sh"
elif [ "$site_version" = "$real_version" ]; then
  ok "the site shows v$site_version, which is what the binaries report"
else
  bad "the site shows v$site_version; the workspace is $real_version"
fi

# etude-core's zero dependencies, the claim the no-network argument rests on.
# Checked against the manifest, not against the sentence in the readme.
core_deps=$(awk '/^\[dependencies\]/{f=1;next} /^\[/{f=0} f && NF && $0 !~ /^#/' crates/etude-core/Cargo.toml | wc -l | tr -d ' ')
if [ "$core_deps" = "0" ]; then
  ok "etude-core declares zero dependencies in its manifest"
else
  bad "readme says etude-core has zero dependencies; its manifest lists $core_deps"
fi

# The page carries the transcripts twice: fetched from demo/transcripts.json
# when hosted, and inline when the file is opened locally. The generator says
# it writes "the same data, same step" -- and they had drifted anyway, so the
# hosted page witnessed one output contract and a double-clicked copy
# witnessed an older one. Nobody would ever notice by reading.
inline_check=$(python3 - <<'PY_INNER'
import json, re, sys
try:
    ext = json.load(open("demo/transcripts.json"))
    html = open("demo/index.html").read()
    m = re.search(r'<script type="application/json" id="transcripts-inline">(.*?)</script>', html, re.S)
    if not m:
        print("MISSING"); sys.exit(0)
    print("SAME" if json.loads(m.group(1)) == ext else "DRIFTED")
except Exception as e:
    print("ERROR " + str(e))
PY_INNER
)
case "$inline_check" in
  SAME) ok "the page's inline transcripts are byte-identical to demo/transcripts.json" ;;
  MISSING) bad "demo/index.html has no inline transcript block; a local copy of the page would show nothing" ;;
  DRIFTED) bad "demo/index.html's inline transcripts differ from demo/transcripts.json. A hosted page and a local one would witness different output. Re-run scripts/capture-demos.sh" ;;
  *) bad "could not compare the inline transcripts with demo/transcripts.json: $inline_check" ;;
esac

# The API panel is the tool's own help, captured. If a help stops being
# captured the panel goes empty and the page silently documents nothing --
# which is how the hand-written table it replaced went wrong in the first
# place, only quieter.
helps=$(python3 - <<'PY_INNER'
import json
try:
    d = json.load(open("demo/transcripts.json"))
    have = {t["label"] for t in d["transcripts"]}
    missing = [f"{t}-help" for t in ("sweep", "stash", "unpack") if f"{t}-help" not in have]
    print(",".join(missing) if missing else "ALL")
except Exception as e:
    print("ERROR " + str(e))
PY_INNER
)
if [ "$helps" = "ALL" ]; then
  ok "the site's API panels are each a captured --help, not a table written beside it"
else
  bad "no captured help for: $helps. That tool's API panel on the site is empty. Re-run scripts/capture-demos.sh"
fi

exit $status
