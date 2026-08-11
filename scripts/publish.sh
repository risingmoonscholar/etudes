#!/usr/bin/env bash
# Everything that has to be true before this repo is public, then the metadata
# GitHub shows in search, on a profile, and in the sidebar.
#
#   scripts/publish.sh --check    run the gates, change nothing   (default)
#   scripts/publish.sh --go       set metadata and make it public
#
# It refuses to publish if any gate fails. The point of this project is that a
# claim comes with the command that checks it, so shipping it with a failing
# check would be the joke telling itself.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

REPO="risingmoonscholar/etudes"
DESC="Three command-line tools that tidy a folder without reading your private files"
TOPICS="rust,cli,macos,privacy,local-first,file-management,command-line-tool"
HOMEPAGE="https://rohanbhatt.com"

MODE="${1:---check}"
fail=0
step() { printf "  %-42s" "$1"; }
ok()   { echo "ok"; }
bad()  { echo "FAILED"; fail=1; }

echo "gates"
step "fmt";            cargo fmt --all --check >/dev/null 2>&1 && ok || bad
step "clippy";         cargo clippy --all-targets --all-features -- -D warnings >/dev/null 2>&1 && ok || bad
step "tests";          cargo test --all >/dev/null 2>&1 && ok || bad
step "claims match";   bash scripts/check-claims.sh >/dev/null 2>&1 && ok || bad
step "no-network";     bash scripts/no-network-test.sh >/dev/null 2>&1 && ok || bad
step "release build";  cargo build --release >/dev/null 2>&1 && ok || bad

echo "hygiene"
step "working tree clean"
[ -z "$(git status --porcelain)" ] && ok || bad
step "nothing unpushed"
[ "$(git log --oneline @{u}..HEAD 2>/dev/null | wc -l | tr -d ' ')" = "0" ] && ok || bad
step "no claude signature in history"
[ "$(git log --all --format='%B%an %ae' | grep -ci 'claude\|co-authored')" = "0" ] && ok || bad
step "no private project names"
! git grep -qiE 'segno|trisolaris|ad astra' -- . >/dev/null 2>&1 && ok || bad

if [ "$fail" != "0" ]; then
  echo ""
  echo "not publishing: something above failed."
  exit 1
fi

echo ""
echo "would set"
echo "  description  $DESC"
echo "  topics       $TOPICS"
echo "  homepage     $HOMEPAGE"

if [ "$MODE" != "--go" ]; then
  echo ""
  echo "checks only. re-run with --go to set metadata and make it public."
  exit 0
fi

echo ""
gh repo edit "$REPO" --description "$DESC" --homepage "$HOMEPAGE" \
  $(printf -- "--add-topic %s " ${TOPICS//,/ }) && echo "  metadata set"
gh repo edit "$REPO" --visibility public --accept-visibility-change-consequences && echo "  now public"

echo ""
echo "one thing left, and it has never been tested against a public url:"
echo "  cargo install --git https://github.com/$REPO sweep-cli"
