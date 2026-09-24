#!/usr/bin/env bash
# The GIF is a live terminal capture. This script creates only synthetic files.
set -euo pipefail
bin="${ETUDES_DEMO_BIN:?}"
work="${ETUDES_DEMO_WORK:?temporary recording directory required}"
root="$work/Desktop"
count="${STASH_DEMO_FILES:-3000}"
mkdir -p "$root"

python3 - "$root" "$count" <<'PY'
import os, sys
root, count = sys.argv[1], int(sys.argv[2])
old = 1767225600
for n in range(1, count + 1):
    path = os.path.join(root, f"screen-share-item-{n:04d}.txt")
    with open(path, "w", encoding="utf-8") as f:
        f.write(f"SYNTHETIC DEMO ITEM {n}\n")
    os.utime(path, (old, old))
with open(os.path.join(root, "tax_return_2025.pdf"), "w", encoding="utf-8") as f:
    f.write("SYNTHETIC DEMO FILE — NOT REAL DATA\n")
with open(os.path.join(root, ".hidden-note"), "w", encoding="utf-8") as f:
    f.write("Hidden demo marker stays in place.\n")
PY

cd "$root"
export ETUDE_STATE_DIR=../s
printf '\033[2m0.5.3 candidate · clearing a synthetic screen-share folder\033[0m\n'
printf '\033[33m$ stash . --for 3d\033[0m\n\n'
"$bin/stash" . --for 3d

printf '\n\033[33m$ stash pop\033[0m\n\n'
"$bin/stash" pop

restored="$(find . -maxdepth 1 -type f ! -name '.hidden-note' | wc -l | tr -d ' ')"
test "$restored" = "$((count + 1))"
test -f "$root/.hidden-note"
printf '\n\033[2mVerified: %s visible items restored; hidden marker never moved.\033[0m\n' "$restored"
