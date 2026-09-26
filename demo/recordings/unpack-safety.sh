#!/usr/bin/env bash
# The GIF is a live terminal capture. Both archives and all contents are synthetic.
set -euo pipefail
bin="${ETUDES_DEMO_BIN:?}"
work="${ETUDES_DEMO_WORK:?temporary recording directory required}"
mkdir -p "$work/good" "$work/bad"
printf 'synthetic\n' > "$work/good/Report.pdf"
printf 'synthetic\n' > "$work/good/café_menu.pdf"
printf 'SYNTHETIC TEST FIXTURE - NOT REAL DATA\n' > "$work/bad/tax_return_2023_filed.pdf"
mkdir -p "$work/outside"
printf 'SYNTHETIC\n' > "$work/outside/secret_outside.txt"
ln -s "$work/outside/secret_outside.txt" "$work/bad/escape_link"
(cd "$work/good" && zip -q "$work/site_export.zip" Report.pdf café_menu.pdf)
(cd "$work/bad" && zip -q -y "$work/photos_2025.zip" tax_return_2023_filed.pdf escape_link)

cd "$work"
export ETUDE_STATE_DIR="$work/state"
printf '\033[2m~/Desktop · synthetic fixture · unpack 0.5.3 candidate\033[0m\n'
printf '\033[33m$ unpack site_export.zip --into site_export\033[0m\n\n'
"$bin/unpack" site_export.zip --into site_export
sleep 1.4

printf '\n\033[33m$ unpack photos_2025.zip --into photos_2025\033[0m\n\n'
set +e
"$bin/unpack" photos_2025.zip --into photos_2025
status="$?"
set -e
test "$status" -eq 2
test ! -e "$work/photos_2025"
test "$(cat "$work/outside/secret_outside.txt")" = "SYNTHETIC"
test -f "$work/site_export/Report.pdf"
sleep 1.2
