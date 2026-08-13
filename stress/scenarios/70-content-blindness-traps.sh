#!/usr/bin/env bash
# Content-blindness traps: facsimile files whose CONTENT contradicts their NAME.
#
# Every scan banner claims "no contents read". This scenario makes that claim
# falsifiable rather than trusted. Each trap routes one way by name and the
# opposite way by content, so a sweep that ever reads a file routes it wrongly
# and the trap fires.
#
# Two of the traps are unfakeable:
#
#   FIFO       reading a named pipe with no writer blocks forever. A sweep
#              that opens contents hangs. Completing is the proof.
#   INSTRUMENT the same .txt trap MUST fire under --inspect-content. A trap
#              that can never fire proves nothing, so the positive arm is
#              part of the scenario rather than an afterthought.
#
# All content is fabricated facsimile. No real record of anyone's exists here.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
D="$W/Desktop"
mkdir -p "$D"

# --- Trap 1: innocent name, personal-record content, in a format sweep never
#     inspects (.jpg). Name says holiday photo; body is a fabricated 1099.
#     Correct behaviour: grouped with the other photos, by name.
cat > "$D/bali_sunset_07.jpg" <<'TRAP'
First Fictional Bank N.A.  --  Annual Statement
Statement period: 01 Jan 2025 to 31 Dec 2025
Account number: 0000-FICTIONAL-0000
Routing number: 000000000
Available balance: $1,234.56
Taxable income reported: $1,234.56
Withholding: $0.00
Fabricated facsimile for a test fixture. Not a real record.
TRAP
for i in 1 2 3 4; do : > "$D/bali_sunset_0$i.jpg"; done

# --- Trap 2: the inverse. Personal-record name, junk content.
#     Correct behaviour: refused by name regardless of what is inside.
echo "lorem ipsum junk bytes nothing sensitive here" > "$D/1099-INT_fictional_bank.pdf"

# --- Trap 3: credential-shaped name, empty file. Refused by name or not at all.
: > "$D/passport_scan_page1.png"

# --- Trap 4 (the instrument): innocent .txt name, personal-record content, in
#     a format --inspect-content DOES read. This is the one trap that must
#     behave differently in the two arms, which is what proves the harness can
#     detect a content read at all.
cat > "$D/team_offsite_agenda.txt" <<'TRAP'
Second Fictional Savings  --  Annual Statement
Statement period: 01 Jan 2025 to 31 Dec 2025
Account number: 0000-FICTIONAL-7788
Routing number: 000000000
Available balance: $4,321.00
Taxable income reported: $4,321.00
Withholding: $0.00
Fabricated facsimile for a test fixture. Not a real record.
TRAP
# Siblings, so the trap file has a name-group to belong to. Without them it
# lands in "no clear group" and the blind arm proves nothing about routing.
# Five siblings: a group needs five members to form.
for i in 1 2 3 4 5; do : > "$D/team_offsite_notes_$i.txt"; done

for i in 1 2 3 4 5; do : > "$D/Screenshot 2026-08-1$i at 09.00.00.png"; done

json=$("$SWEEP" "$D" --json 2>&1)

grouped_by_name() {  # grouped_by_name NEEDLE
  python3 -c "
import json,sys
d=json.load(sys.stdin)
m=[f for g in d['groups'] for f in g.get('members',[])]
print(any('$1' in f for f in m))
" <<<"$json"
}
personal_count=$(python3 -c "
import json,sys; print(json.load(sys.stdin)['left_alone']['looks_personal'])" <<<"$json")

assert_eq "True" "$(grouped_by_name bali_sunset_07)" \
  "TRAP 1: 1099 content inside a .jpg name is grouped BY NAME with the photos"
assert_eq "True" "$(grouped_by_name team_offsite_agenda)" \
  "TRAP 4 (blind arm): 1099 content inside a .txt name is grouped BY NAME with its siblings, not refused"
assert_eq 2 "$personal_count" \
  "TRAP 2+3: only the two personal-NAMED files are refused; neither content-contradicting file is"

# --- Trap 5: a FIFO. Any open()+read of contents blocks forever on a pipe with
#     no writer. There is no `timeout` on macOS, so run it in the background
#     and poll with a deadline. A hang is the trap firing.
if mkfifo "$D/quarterly_summary.txt" 2>/dev/null; then
  "$SWEEP" "$D" --json >/dev/null 2>&1 &
  pid=$!
  waited=0
  while kill -0 "$pid" 2>/dev/null && [ "$waited" -lt 15 ]; do
    sleep 1; waited=$((waited+1))
  done
  if kill -0 "$pid" 2>/dev/null; then
    kill -9 "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
    fail "TRAP 5 FIRED: sweep hung ${waited}s on a FIFO. It opened file contents"
  else
    wait "$pid" 2>/dev/null
    pass "TRAP 5: FIFO present, scan finished in ${waited}s. No content was opened"
  fi
else
  unproven "TRAP 5: FIFO trap" "mkfifo unavailable on this filesystem"
fi

# --- INSTRUMENT ARM 1: prove the .txt trap CAN fire. Under --inspect-content
#     it must move out of its group. A trap that can never fire proves nothing.
#     Runs in its own tree: the FIFO above would block a content-reading scan,
#     which is correct behaviour and the subject of arm 2, but it would hang
#     this arm before it could assert anything.
I="$W/Inspect"; mkdir -p "$I"
cp "$D/team_offsite_agenda.txt" "$I/"
for i in 1 2 3 4 5; do : > "$I/team_offsite_notes_$i.txt"; done

if command -v script >/dev/null 2>&1; then
  # Consent is read from the tty after the terms print. A pipe that closes at
  # once reads as EOF, EOF is not consent, and sweep then continues name-only --
  # so the arm would silently test nothing and pass. Hold stdin open past the
  # prompt.
  insp=$({ sleep 1; echo y; sleep 3; } \
         | script -q /dev/null "$SWEEP" "$I" --inspect-content --json 2>/dev/null \
         | tr -d '\r' | grep -o '{.*}' | tail -1)
  if [ -n "$insp" ]; then
    still_grouped=$(python3 -c "
import json,sys
try:
    d=json.loads(sys.stdin.read())
    m=[f for g in d['groups'] for f in g.get('members',[])]
    print(any('team_offsite_agenda' in f for f in m))
except Exception:
    print('PARSE-FAIL')
" <<<"$insp")
    case "$still_grouped" in
      False) pass "INSTRUMENT 1: under --inspect-content the .txt trap FIRES (leaves its group). The name-routing traps can detect a content read" ;;
      True)  fail "INSTRUMENT 1: consent given but the .txt trap did not fire. These traps cannot detect a content read, so their green proves nothing" ;;
      *)     unproven "INSTRUMENT 1" "could not parse the --inspect-content plan" ;;
    esac
  else
    unproven "INSTRUMENT 1" "could not drive consent through script(1)"
  fi
else
  unproven "INSTRUMENT 1" "script(1) not available to fake a tty"
fi

# --- INSTRUMENT ARM 2: prove the FIFO trap is a real detector, not a decoration.
#     Trap 5 shows a blind scan finishes. That is only evidence if a scan that
#     DOES read contents hangs on the same pipe. Same directory, same FIFO,
#     content reading on: it must NOT finish.
F="$W/Fifo"; mkdir -p "$F"
for i in 1 2 3 4 5; do : > "$F/quarter_report_$i.txt"; done
if mkfifo "$F/quarter_report_9.txt" 2>/dev/null && command -v script >/dev/null 2>&1; then
  { sleep 1; echo y; sleep 30; } \
    | script -q /dev/null "$SWEEP" "$F" --inspect-content --json >/dev/null 2>&1 &
  ipid=$!
  w=0
  while kill -0 "$ipid" 2>/dev/null && [ "$w" -lt 12 ]; do sleep 1; w=$((w+1)); done
  if kill -0 "$ipid" 2>/dev/null; then
    pkill -P "$ipid" 2>/dev/null; kill -9 "$ipid" 2>/dev/null; wait "$ipid" 2>/dev/null
    pass "INSTRUMENT 2: with --inspect-content the same FIFO BLOCKS the scan. Trap 5's completion is therefore evidence, not decoration"
  else
    wait "$ipid" 2>/dev/null
    fail "INSTRUMENT 2: a content-reading scan finished over a FIFO. The FIFO trap cannot detect a content read, so trap 5 proves nothing"
  fi
else
  unproven "INSTRUMENT 2: FIFO positive control" "mkfifo or script(1) unavailable"
fi
