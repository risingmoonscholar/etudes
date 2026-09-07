#!/usr/bin/env bash
# A working Desktop, mid-project. The shape a real one has after a month:
# a screenshot habit, a camera dump, installers nobody deleted, one client's
# assets, and private documents sitting in the same folder as everything else.
#
# Nothing here is real. The tree is generated and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
D="$W/Desktop"; mkdir -p "$D"

# The habit: screenshots accumulate daily.
for i in $(seq 1 60); do
  : > "$D/Screenshot 2026-0$((i%9+1))-$(printf %02d $((i%28+1))) at $(printf %02d $((i%12+1))).30.11 AM.png"
done
# The camera dump.
for i in $(seq 5100 5180); do : > "$D/IMG_$i.HEIC"; done
# Installers nobody deleted.
for f in Docker-4.28.0.dmg Figma-124.5.dmg node-v22.1.0.pkg Zoom.pkg; do : > "$D/$f"; done
# One client's assets.
for f in northwind_logo_v4.psd northwind_brand.pdf northwind_type.sketch northwind_deck.key; do : > "$D/$f"; done
# Private documents, in the same folder as everything else. These must not move.
PRIVATE=(W2_2025_northwind.pdf passport_scan.png lab_results_panel.pdf id_rsa recovery_codes.txt bank_statement_may.pdf)
for f in "${PRIVATE[@]}"; do : > "$D/$f"; done
# Work in progress that groups by nothing.
for f in "Untitled 7.pdf" final_v3_ACTUALLY_final.docx scratch.txt; do : > "$D/$f"; done

BEFORE=$(find "$D" -type f | wc -l | tr -d ' ')

# A plan changes nothing.
assert_exit 0 "planning a busy Desktop succeeds" -- "$SWEEP" "$D"
assert_intact "$D" "$BEFORE" "planning moved nothing"

# The private documents survive an apply that the user consented to.
"$SWEEP" apply "$D" --yes >/dev/null 2>&1
missing=0
for f in "${PRIVATE[@]}"; do [ -f "$D/$f" ] || missing=$((missing+1)); done
assert_eq 0 "$missing" "every private document stayed where it was"

# Nothing was destroyed: the count is the same, wherever things now live.
AFTER=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq "$BEFORE" "$AFTER" "no file was lost by the apply"

# And it is reversible.
"$SWEEP" undo >/dev/null 2>&1
RESTORED=$(find "$D" -maxdepth 1 -type f | wc -l | tr -d ' ')
assert_eq "$BEFORE" "$RESTORED" "undo returned every file to the Desktop"
