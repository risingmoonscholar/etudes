#!/usr/bin/env bash
# The GIF is a live terminal capture. Both archives and all contents are synthetic.
set -euo pipefail
bin="${ETUDES_DEMO_BIN:?}"
work="${ETUDES_DEMO_WORK:?temporary recording directory required}"
mkdir -p "$work/good" "$work/bad"
printf 'SYNTHETIC DEMO NOTES\n' > "$work/good/notes.txt"
printf 'SYNTHETIC DEMO PHOTO PLACEHOLDER\n' > "$work/good/photo.jpg"
printf 'SYNTHETIC DEMO FILE — NOT REAL DATA\n' > "$work/bad/notes.txt"
printf 'SYNTHETIC OUTSIDE SENTINEL\n' > "$work/outside-sentinel.txt"
ln -s "$work/outside-sentinel.txt" "$work/bad/shortcut"
(cd "$work/good" && zip -q "$work/welcome.zip" notes.txt photo.jpg)
(cd "$work/bad" && zip -q -y "$work/unsafe.zip" notes.txt shortcut)

cd "$work"
printf '\033[2m0.5.3 candidate · a normal archive and a dangerous one\033[0m\n'
printf '\033[33m$ unpack welcome.zip --into welcome\033[0m\n\n'
"$bin/unpack" welcome.zip --into welcome
sleep 1.4

printf '\n\033[33m$ unpack unsafe.zip --into unsafe\033[0m\n\n'
set +e
"$bin/unpack" unsafe.zip --into unsafe
status="$?"
set -e
test "$status" -eq 2
test ! -e "$work/unsafe"
test "$(cat "$work/outside-sentinel.txt")" = "SYNTHETIC OUTSIDE SENTINEL"
test -f "$work/welcome/notes.txt"
sleep 1.2
printf '\n\033[2mVerified: safe files extracted; unsafe archive refused before writing.\033[0m\n'
