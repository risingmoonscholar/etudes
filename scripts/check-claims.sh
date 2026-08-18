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
readme_tag=$(grep -oE '\-\-tag v[0-9]+\.[0-9]+\.[0-9]+' README.md | head -1 | grep -oE 'v[0-9.]+')
newest_tag=$(git tag --sort=-v:refname | head -1)
if [ -z "$newest_tag" ]; then
  ok "no tags yet, so the readme pins nothing"
elif [ "$readme_tag" = "$newest_tag" ]; then
  ok "README.md installs $readme_tag, the newest tag"
else
  bad "README.md installs ${readme_tag:-nothing}; the newest tag is $newest_tag"
fi

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
real_version=$(grep -m1 '^version' Cargo.toml | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
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

exit $status
