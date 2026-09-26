#!/usr/bin/env bash
# The GIF is a live terminal capture. This script creates only synthetic files.
set -euo pipefail
bin="${ETUDES_DEMO_BIN:?}"
work="${ETUDES_DEMO_WORK:?temporary recording directory required}"
root="$work/Desktop"
count="${SWEEP_DEMO_FILES:-9999}"
mkdir -p "$root"

python3 - "$root" "$count" <<'PY'
import os, sys
root, count = sys.argv[1], int(sys.argv[2])
old = 1767225600  # 2026-01-01, outside sweep's one-day grace window
for n in range(1, count + 1):
    path = os.path.join(root, f"IMG_{4400 + n - 1}.HEIC")
    fd = os.open(path, os.O_CREAT | os.O_WRONLY | os.O_EXCL, 0o600)
    os.close(fd)
    os.utime(path, (old, old))
private = os.path.join(root, "tax_return_2023_filed.pdf")
with open(private, "w", encoding="utf-8") as f:
    f.write("SYNTHETIC TEST FIXTURE - NOT REAL DATA\n")
os.utime(private, (old, old))
PY

cd "$root"
export ETUDE_STATE_DIR=../s
printf '\033[2m~/Desktop · synthetic fixture · sweep 0.5.3 candidate\033[0m\n'
printf '\033[33m$ sweep apply ~/Desktop --yes\033[0m\n\n'
"$bin/sweep" apply . --yes

printf '\n\033[33m$ sweep undo ~/Desktop\033[0m\n\n'
"$bin/sweep" undo

restored="$(find . -maxdepth 1 -type f -name 'IMG_*.HEIC' | wc -l | tr -d ' ')"
test "$restored" = "$count"
test -f "$root/tax_return_2023_filed.pdf"
