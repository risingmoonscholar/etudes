#!/usr/bin/env bash
# Decide whether a CI event must run the adversarial stress suite.
#
# Usage: changed-paths | scripts/stress-ci-scope.sh EVENT
# Output is GitHub-output-compatible key=value lines.  The allowed skip set is
# deliberately small: uncertainty must run the suite, never skip it.
set -euo pipefail

decide() {
  local event="$1" changed
  changed=$(cat)
  if [ "$event" != pull_request ]; then
    printf 'run=yes\nreason=event is %s, not a pull request\n' "$event"
  elif printf '%s\n' "$changed" | command grep -qvE '^(README\.md|CHANGELOG\.md|CONTRIBUTING\.md|SECURITY\.md|LICENSE|\.gitignore)$'; then
    printf 'run=yes\nreason=the diff touches something outside documentation\n'
  else
    printf 'run=no\nreason=every changed path is documentation\n'
  fi
}

if [ "${1:-}" = --self-test ]; then
  doc=$(printf 'README.md\nCHANGELOG.md\n' | decide pull_request)
  code=$(printf 'src/main.rs\n' | decide pull_request)
  scheduled=$(printf 'README.md\n' | decide schedule)
  [ "$doc" = $'run=no\nreason=every changed path is documentation' ]
  [ "$code" = $'run=yes\nreason=the diff touches something outside documentation' ]
  [ "$scheduled" = $'run=yes\nreason=event is schedule, not a pull request' ]
  echo 'ok stress CI scope: docs skip visibly; code and scheduled runs execute'
  exit 0
fi

[ "$#" -eq 1 ] || { echo "usage: $0 EVENT" >&2; exit 2; }
decide "$1"
