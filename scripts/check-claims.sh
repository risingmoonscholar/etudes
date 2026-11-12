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
#     grep -oE '[0-9]+ scenarios' README.md
#
# -- so when the count went from 33 to 36, the pattern stopped matching, the
# result was empty, and an `if [ -n "$claimed" ]` guard skipped the check
# without a word. The readme was wrong by three and this script reported green.
# A checker that only finds the claim while the claim is already correct is
# not a checker. Every lookup below fails loudly when it finds nothing.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

status=0
# Sourced, not required: a check that cannot emit a trace still checks.
# shellcheck source=/dev/null
[ -f "$(dirname "${BASH_SOURCE[0]}")/trace.sh" ] && . "$(dirname "${BASH_SOURCE[0]}")/trace.sh"
type trace_event >/dev/null 2>&1 || trace_event() { :; }

# Each verdict this script already prints is also emitted as a typed event.
# The trace is a byproduct of the check, never a separate task.
ok()   { printf "ok   %s\n" "$1"; trace_event verification "$1" ; }
bad()  { printf "FAIL %s\n" "$1"; status=1; trace_event failure "$1" ; }

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

if ! tests_output=$(cargo test --all 2>&1); then
  bad "cargo test --all failed (including the binary capability-contract witnesses). Counts cannot certify a failed suite."
  printf '%s\n' "$tests_output" | tail -60
  exit 1
fi
tests_actual=$(printf '%s\n' "$tests_output" | grep -oE '[0-9]+ passed' | awk '{s+=$1} END {print s}')
[ -z "$tests_actual" ] && { echo "FAIL could not count tests; the suite did not report"; exit 1; }

# README.md states no counts; docs/testing.md and the site carry them. A
# count that comes back into README.md must be checked too, so its return is a
# failure here until this list includes README.md again.
for f in docs/testing.md site/demo/index.html; do
  claim "$f" '[0-9]+ tests' '[0-9]+' "tests" "$tests_actual"
done
if grep -qE '[0-9]+ (tests|scenarios)\b' README.md; then
  bad "README.md states a test or scenario count, which this script does not check there. Add README.md to the count lists here and in sync-claims.sh, or remove the count."
else
  ok "README.md states no test or scenario count; docs/testing.md and the site carry them"
fi

# Scenario count, from what git TRACKS rather than what happens to be sitting
# in the working tree. A reader gets what a clone contains, so an untracked
# scenario is not one the docs can claim. Counting the filesystem here was
# a bug in the first version of this fix: it read 36 with one file untracked,
# so the docs were corrected to a number no clone would ever see.
scenarios_actual=$(git ls-files 'stress/scenarios/*.sh' | wc -l | tr -d ' ')
# Every file that states the number, not just docs/testing.md. site/demo/index.html is
# the published site: it can be wrong in front of everyone while CI is green,
# which is the exact failure this script exists to catch, one file over.
for f in docs/testing.md site/demo/index.html; do
  claim "$f" '[0-9]+ scenarios' '[0-9]+' "scenarios" "$scenarios_actual"
done

# The journal TTL: defined once in code, restated in the changelog.
ttl_actual=$(grep -oE 'TTL_DAYS: u64 = [0-9]+' crates/etude-core/src/journal.rs | grep -oE '[0-9]+$')
[ -z "$ttl_actual" ] && { echo "FAIL could not read TTL_DAYS from journal.rs"; exit 1; }
claim CHANGELOG.md 'pruned after [0-9]+ days' '[0-9]+' "TTL days" "$ttl_actual"

# Internal consistency only: the advertised pin matches its CLI manifest.
# This does not establish publication. check-release.py --published verifies
# remote tag existence, installation and the installed binary's behavior.
# Candidate PRs may name pending tags; README must identify them as candidates.
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
    ok "README.md pin for $crate matches manifest v$ver (publication not checked)"
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
    d=json.load(open('site/transcripts.json'))
    print(d.get('version',''))
except Exception:
    print('')
")
# Per tool, against each tool's own manifest. One ambient version compared
# against one manifest let the site show stash and unpack at a version they
# are frozen below -- the freeze has to convey all the way to the page.
for tool in sweep stash unpack; do
  panel_v=$(python3 -c "
import json
d=json.load(open('site/transcripts.json'))
print(d.get('versions',{}).get('$tool',''))")
  manifest_v=$(grep -m1 '^version' "crates/$tool-cli/Cargo.toml" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
  if [ -z "$panel_v" ]; then
    bad "the site records no version for $tool. Re-run scripts/capture-demos.sh"
  elif [ "$panel_v" = "$manifest_v" ]; then
    ok "the site shows $tool v$panel_v, which its manifest declares"
  else
    bad "the site shows $tool v$panel_v; its manifest declares $manifest_v"
  fi
done
real_version=$(grep -m1 '^version' crates/sweep-cli/Cargo.toml | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
if [ -z "$site_version" ]; then
  bad "site/transcripts.json records no version; the site cannot show one. Re-run scripts/capture-demos.sh"
elif [ "$site_version" = "$real_version" ]; then
  ok "the site shows v$site_version, which is what the binaries report"
else
  bad "the site shows v$site_version; the workspace is $real_version"
fi

# etude-core's zero dependencies, the claim the no-network argument rests on.
# Checked against the manifest, not against the sentence in docs/testing.md
# and SECURITY.md.
core_deps=$(awk '/^\[dependencies\]/{f=1;next} /^\[/{f=0} f && NF && $0 !~ /^#/' crates/etude-core/Cargo.toml | wc -l | tr -d ' ')
if [ "$core_deps" = "0" ]; then
  ok "etude-core declares zero dependencies in its manifest"
else
  bad "docs/testing.md and SECURITY.md say etude-core has zero dependencies; its manifest lists $core_deps"
fi

# The page carries the transcripts twice: fetched from site/transcripts.json
# when hosted, and inline when the file is opened locally. The generator says
# it writes "the same data, same step" -- and they had drifted anyway, so the
# hosted page witnessed one output contract and a double-clicked copy
# witnessed an older one. Nobody would ever notice by reading.
inline_check=$(python3 - <<'PY_INNER'
import json, re, sys
try:
    ext = json.load(open("site/transcripts.json"))
    html = open("site/demo/index.html").read()
    m = re.search(r'<script type="application/json" id="transcripts-inline">(.*?)</script>', html, re.S)
    if not m:
        print("MISSING"); sys.exit(0)
    print("SAME" if json.loads(m.group(1)) == ext else "DRIFTED")
except Exception as e:
    print("ERROR " + str(e))
PY_INNER
)
case "$inline_check" in
  SAME) ok "the page's inline transcripts are byte-identical to site/transcripts.json" ;;
  MISSING) bad "site/demo/index.html has no inline transcript block; a local copy of the page would show nothing" ;;
  DRIFTED) bad "site/demo/index.html's inline transcripts differ from site/transcripts.json. A hosted page and a local one would witness different output. Re-run scripts/capture-demos.sh" ;;
  *) bad "could not compare the inline transcripts with site/transcripts.json: $inline_check" ;;
esac

# The API panel is the tool's own help, captured. If a help stops being
# captured the panel goes empty and the page silently documents nothing --
# which is how the hand-written table it replaced went wrong in the first
# place, only quieter.
helps=$(python3 - <<'PY_INNER'
import json
try:
    d = json.load(open("site/transcripts.json"))
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

# Each tool's capability declaration version, read from contract.rs, against
# every page that names the current one. The 0.5.5 release pages said 17
# (sweep) and 14 (stash) after the code had moved to 18 and 15.
contract=crates/etude-cli-support/src/contract.rs
base=$(grep -oE 'pub const SCHEMA_VERSION: u32 = [0-9]+' "$contract" | grep -oE '[0-9]+$')
for tool in sweep stash unpack; do
  Tool="$(tr '[:lower:]' '[:upper:]' <<<"${tool:0:1}")${tool:1}"
  expr=$(grep -oE "Tool::$Tool => (SCHEMA_VERSION \+ )?[0-9]+," "$contract" | head -1)
  if [ -z "$expr" ] || [ -z "$base" ]; then
    bad "could not read the $tool declaration version from $contract"; continue
  fi
  decl=$(grep -oE '[0-9]+' <<<"$expr" | tail -1)
  case "$expr" in *SCHEMA_VERSION*) decl=$((base + decl)) ;; esac
  claim docs/contracts/README.md "\`v[0-9]+/$tool\.json\`.*are the\$" '[0-9]+' "current $tool declaration pin" "$decl"
  case $tool in
    stash) claim docs/contracts/README.md 'declaration is version [0-9]+ and its surrounding' '[0-9]+' "current stash declaration" "$decl"
           claim docs/contracts/README.md 'declaration is version [0-9]+ in `v[0-9]+/stash' '[0-9]+' "current stash declaration" "$decl"
           claim docs/contracts/README.md 'declaration is version [0-9]+ in `v[0-9]+/stash' 'v[0-9]+' "current stash pin" "v$decl" ;;
    unpack) claim docs/contracts/README.md 'under `details.schema_version` is version [0-9]+' '[0-9]+$' "current unpack declaration" "$decl"
            claim docs/contracts/README.md '`v[0-9]+/unpack\.json` pins the' 'v[0-9]+' "current unpack pin" "v$decl"
            claim docs/unpack.md 'unpack capability declaration is version [0-9]+' '[0-9]+$' "current unpack declaration" "$decl"
            claim docs/unpack.md '`docs/contracts/v[0-9]+/unpack\.json` is the canonical pin' 'v[0-9]+' "current unpack pin" "v$decl" ;;
  esac
  # A tagged release's page describes what shipped; a later bump leaves it alone.
  ver=$(grep -m1 '^version' "crates/$tool-cli/Cargo.toml" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
  page="site/releases/$tool-v$ver.html"
  if git rev-parse -q --verify "refs/tags/$tool-v$ver" >/dev/null; then
    ok "$tool v$ver is tagged; its release page describes what shipped, so its declaration is not checked"
  elif [ ! -f "$page" ]; then
    ok "$tool v$ver has no release page yet, so no page states its declaration"
  else
    claim "$page" "$tool capability declaration is version [0-9]+" '[0-9]+$' "$tool declaration version" "$decl"
  fi
done

exit $status
