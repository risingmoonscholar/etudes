#!/usr/bin/env bash
# The default macOS volume (APFS, case-insensitive/case-preserving) means
# "Report.pdf" and "report.PDF" in two different subfolders are two
# different strings but the same destination once both are swept into the
# same group directory. sweep's destination-collision check was written to
# catch this by lowercasing before comparing (see apply.rs). This proves it,
# end to end, against a real APFS directory rather than trusting the source.
#
# This is the positive control for 20-unicode-nfc-nfd-collision.sh: same
# shape of attack, ASCII case only. If this ever goes red, the regression is
# in the collision check itself, not in Unicode handling.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
D="$W/Desktop"
mkdir -p "$D/ClientA" "$D/ClientB"

echo "ORIGINAL-CONTENT-FROM-CLIENT-A" > "$D/ClientA/invoice_Report.pdf"
echo "ORIGINAL-CONTENT-FROM-CLIENT-B" > "$D/ClientB/invoice_report.PDF"
for i in 0 1 2; do : > "$D/invoice_filler_$i.pdf"; done

BEFORE=$(find "$D" -type f | wc -l | tr -d ' ')
assert_eq 5 "$BEFORE" "fixture built: 5 files, two of them the same name differing only in case"

assert_exit 0 "planning a tree with a case-only name pair succeeds" -- "$SWEEP" "$D" --depth 2

assert_exit 2 "apply refuses the case-collision cleanly (DestinationCollision)" \
  -- "$SWEEP" apply "$D" --depth 2 --yes

assert_intact "$D" "$BEFORE" "nothing moved — the refusal was pre-flight, not partial"
[ -f "$D/ClientA/invoice_Report.pdf" ] && pass "ClientA's file is exactly where it started" \
  || fail "ClientA's file moved despite the refusal"
[ -f "$D/ClientB/invoice_report.PDF" ] && pass "ClientB's file is exactly where it started" \
  || fail "ClientB's file moved despite the refusal"
