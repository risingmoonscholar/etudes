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
DRAFT="${1:?usage: file-issue.sh [--dry-run] <draft.md> --label <name> [--label <name>...]}"
shift

# Labels are part of the universal standard, like the footer: every issue
# carries at least one, from the repository's existing label set.
LABELS=()
while [ $# -gt 0 ]; do
    case "$1" in
        --label) LABELS+=("$2"); shift 2 ;;
        *) echo "file-issue.sh: unknown argument: $1" >&2; exit 2 ;;
    esac
done
if [ ${#LABELS[@]} -eq 0 ]; then
    echo "file-issue.sh: at least one --label is required; see 'gh label list'" >&2
    exit 2
fi

# The footer is appended before gating, so the gate judges the exact text
# that gets filed.
WORK=$(mktemp)
trap 'rm -f "$WORK"' EXIT
cp "$DRAFT" "$WORK"
grep -q "Filed by Night Watch" "$WORK" || printf '\n---\n\nFiled by Night Watch, an agent running the Witness checks on this repo.\n' >> "$WORK"

python3 scripts/check-issues.py --draft "$WORK"

TITLE=$(head -1 "$WORK" | sed 's/^#* *//')
LABEL_ARGS=()
for l in "${LABELS[@]}"; do LABEL_ARGS+=(--label "$l"); done
if [ $DRY -eq 1 ]; then
    echo "dry-run: would file as: $TITLE [${LABELS[*]}]"
    exit 0
fi
tail -n +2 "$WORK" | gh issue create --title "$TITLE" --body-file - "${LABEL_ARGS[@]}"
