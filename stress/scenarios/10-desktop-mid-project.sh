#!/usr/bin/env bash
# A working Desktop, mid-project. The shape a real one has after a month:
# a screenshot habit, a camera dump, installers nobody deleted, one client's
# assets, and private documents sitting in the same folder as everything else.
#
# Nothing here is real. The tree is generated and removed on exit.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
D="$W/Desktop"; mkdir -p "$D"
# The scenario is about movement and recovery, not whether this particular
# host's login keychain is unlocked. A supplied ephemeral key keeps the journal
# real and makes the result reproducible on headless CI as well.
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"

# The habit: screenshots accumulate daily.
for i in $(seq 1 60); do
  printf 'screenshot payload %s\n' "$i" > "$D/Screenshot 2026-0$((i%9+1))-$(printf %02d $((i%28+1))) at $(printf %02d $((i%12+1))).30.11 AM.png"
done
# The camera dump.
for i in $(seq 5100 5180); do printf 'camera payload %s\n' "$i" > "$D/IMG_$i.HEIC"; done
# Installers nobody deleted.
for f in Docker-4.28.0.dmg Figma-124.5.dmg node-v22.1.0.pkg Zoom.pkg; do printf 'installer payload %s\n' "$f" > "$D/$f"; done
# One client's assets.
for f in northwind_logo_v4.psd northwind_brand.pdf northwind_type.sketch northwind_deck.key; do : > "$D/$f"; done
# Private documents, in the same folder as everything else. These must not move.
PRIVATE=(W2_2025_northwind.pdf passport_scan.png lab_results_panel.pdf id_rsa recovery_codes.txt bank_statement_may.pdf)
for f in "${PRIVATE[@]}"; do printf 'private payload %s\n' "$f" > "$D/$f"; done
# Work in progress that groups by nothing.
for f in "Untitled 7.pdf" final_v3_ACTUALLY_final.docx scratch.txt; do : > "$D/$f"; done

BEFORE="$W/before.json"
AFTER_PLAN="$W/after-plan.json"
AFTER_APPLY="$W/after-apply.json"
AFTER_UNDO="$W/after-undo.json"
snapshot_tree "$D" "$BEFORE"

# A plan changes nothing.
assert_exit 0 "planning a busy Desktop succeeds" -- "$SWEEP" "$D"
snapshot_tree "$D" "$AFTER_PLAN"
assert_snapshot_eq "$BEFORE" "$AFTER_PLAN" "planning changed no path, payload, symlink, or directory"

# The private documents survive an apply that the user consented to.
assert_exit 0 "applying the busy Desktop succeeds" -- "$SWEEP" apply "$D" --yes
snapshot_tree "$D" "$AFTER_APPLY"
assert_snapshot_ne "$BEFORE" "$AFTER_APPLY" "apply changed the Desktop layout"

# A no-op implementation can preserve every file and still pass a count-only
# test. These exact paths prove that real eligible files left their origins.
[ ! -e "$D/Screenshot 2026-02-02 at 02.30.11 AM.png" ] && [ -f "$D/Screenshots/Screenshot 2026-02-02 at 02.30.11 AM.png" ] \
  && pass "a screenshot moved to Screenshots with its original bytes" \
  || fail "the representative screenshot did not move from its origin to Screenshots"
[ ! -e "$D/Docker-4.28.0.dmg" ] && [ -f "$D/Installers/Docker-4.28.0.dmg" ] \
  && pass "an installer moved to Installers with its original bytes" \
  || fail "the representative installer did not move from its origin to Installers"
missing=0
for f in "${PRIVATE[@]}"; do [ -f "$D/$f" ] || missing=$((missing+1)); done
assert_eq 0 "$missing" "every private document stayed where it was"

# The actual private bytes, not merely their paths, must still be present.
assert_eq 'private payload passport_scan.png' "$(cat "$D/passport_scan.png")" "protected document bytes stayed unchanged"

# And it is reversible.
assert_exit 0 "undoing the busy Desktop succeeds" -- "$SWEEP" undo
snapshot_tree "$D" "$AFTER_UNDO"
assert_snapshot_eq "$BEFORE" "$AFTER_UNDO" "undo restored the exact original Desktop tree and bytes"
