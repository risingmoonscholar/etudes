#!/usr/bin/env bash
# The only filing path. A draft goes through the voice gate; a draft that
# does not pass does not get filed. There is no force flag by maintainer
# ruling: the moment somebody believes their issue is the exception is the
# moment the gate exists for. A wrong rule gets fixed in check-issues.py,
# with its fixture updated -- the gate itself is never bypassed.
#
#   scripts/file-issue.sh <draft.md>            gate, then file via gh
#   scripts/file-issue.sh --dry-run <draft.md>  gate only, file nothing
#
# Line 1 of the draft is the title (a markdown heading is fine; the leading
# hashes are stripped). Everything after the first line is the body.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

DRY=0
[ "${1:-}" = "--dry-run" ] && { DRY=1; shift; }
DRAFT="${1:?usage: file-issue.sh [--dry-run] <draft.md>}"

python3 scripts/check-issues.py --draft "$DRAFT"

TITLE=$(head -1 "$DRAFT" | sed 's/^#* *//')
if [ $DRY -eq 1 ]; then
    echo "dry-run: would file as: $TITLE"
    exit 0
fi
tail -n +2 "$DRAFT" | gh issue create --title "$TITLE" --body-file -
