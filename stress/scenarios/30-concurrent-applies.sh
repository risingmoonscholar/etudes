#!/usr/bin/env bash
# Interruption family: two applies at once, same state directory.
#
# Journal ids were recently changed (nanos-pid-counter-hash) specifically to
# avoid same-second collisions between processes. This attacks that directly:
# two `sweep apply` runs, launched in the same instant against two different
# directories, sharing one XDG_STATE_HOME. Two things are checked —
#
#   1. Do the journals collide or cross-contaminate (directory A's moves
#      showing up recorded against directory B, or one process's journal
#      write clobbering the other's)?
#   2. `sweep undo` has no directory argument — it always reverses "the most
#      recent apply". With two applies racing, only one journal can hold that
#      title. Does the CLI's own success message ("Undo with: sweep undo"),
#      printed identically by both processes, silently overpromise for
#      whichever one loses the race?
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
D1="$W/D1"; D2="$W/D2"
mkdir -p "$D1" "$D2"

# Two differently-shaped trees so a cross-contamination check is unambiguous:
# if any IMG_ file ends up filed under D1, or any Screenshot under D2, or a
# file from one tree's baseline ends up inside the other tree at all, that is
# corruption, not coincidence.
for i in $(seq 1 60); do
  : > "$D1/Screenshot 2026-0$((i % 9 + 1))-$(printf %02d $((i % 28 + 1))) at $(printf %02d $((i % 12 + 1))).$(printf %02d $((i % 60))).$(printf %02d $((i % 60))) AM ($i).png"
done
for i in $(seq 4400 4460); do : > "$D2/IMG_$i.HEIC"; done

BEFORE1=$(find "$D1" -type f -exec basename {} \; | sort)
BEFORE2=$(find "$D2" -type f -exec basename {} \; | sort)
N1=$(echo "$BEFORE1" | grep -c .)
N2=$(echo "$BEFORE2" | grep -c .)

"$SWEEP" apply "$D1" --yes >/tmp/conc_d1_out.$$ 2>&1 &
P1=$!
"$SWEEP" apply "$D2" --yes >/tmp/conc_d2_out.$$ 2>&1 &
P2=$!
wait "$P1"; R1=$?
wait "$P2"; R2=$?

assert_eq 0 "$R1" "concurrent apply on directory 1 exited cleanly"
assert_eq 0 "$R2" "concurrent apply on directory 2 exited cleanly"

# --- Cross-contamination: every file is still in ITS OWN tree, nothing lost.
AFTER1=$(find "$D1" -type f -exec basename {} \; | sort)
AFTER2=$(find "$D2" -type f -exec basename {} \; | sort)
CROSS1=$(comm -12 <(echo "$BEFORE2") <(echo "$AFTER1"))
CROSS2=$(comm -12 <(echo "$BEFORE1") <(echo "$AFTER2"))

if [ -z "$CROSS1" ] && [ -z "$CROSS2" ] && [ "$(echo "$AFTER1" | grep -c .)" = "$N1" ] && [ "$(echo "$AFTER2" | grep -c .)" = "$N2" ]; then
  pass "two concurrent applies against different directories, one shared state dir: neither tree lost a file or picked up the other's"
else
  fail "concurrent applies cross-contaminated the trees. D1 gained from D2: [$CROSS1]  D2 gained from D1: [$CROSS2]  D1 file count now $(echo "$AFTER1" | grep -c .)/$N1, D2 now $(echo "$AFTER2" | grep -c .)/$N2"
fi

# --- Journal identity: two distinct, non-colliding journal files. ----------
JCOUNT=$(find "$XDG_STATE_HOME/etudes" -maxdepth 1 -name 'sweep-*.journal' 2>/dev/null | wc -l | tr -d ' ')
assert_eq 2 "$JCOUNT" "two concurrent applies produced two distinct journal files (no id collision clobbered one)"

# --- The undo-reachability trap: both processes print "Undo with: sweep
# undo" — do both promises actually hold?
JID1=$(grep -o 'sweep-[^ ]*\.journal' "/tmp/conc_d1_out.$$" | head -1)
JID2=$(grep -o 'sweep-[^ ]*\.journal' "/tmp/conc_d2_out.$$" | head -1)

"$SWEEP" undo >/tmp/conc_undo1.$$ 2>&1
D1_RESTORED=$([ "$(find "$D1" -maxdepth 1 -type f -exec basename {} \; | sort)" = "$BEFORE1" ] && echo yes || echo no)
D2_RESTORED=$([ "$(find "$D2" -maxdepth 1 -type f -exec basename {} \; | sort)" = "$BEFORE2" ] && echo yes || echo no)
WHICH_FIRST="none"
[ "$D1_RESTORED" = yes ] && WHICH_FIRST=D1
[ "$D2_RESTORED" = yes ] && WHICH_FIRST=D2

"$SWEEP" undo >/tmp/conc_undo2.$$ 2>&1
D1_RESTORED2=$([ "$(find "$D1" -maxdepth 1 -type f -exec basename {} \; | sort)" = "$BEFORE1" ] && echo yes || echo no)
D2_RESTORED2=$([ "$(find "$D2" -maxdepth 1 -type f -exec basename {} \; | sort)" = "$BEFORE2" ] && echo yes || echo no)

if [ "$D1_RESTORED2" = yes ] && [ "$D2_RESTORED2" = yes ]; then
  pass "calling \`sweep undo\` twice after two concurrent applies reversed both directories"
else
  fail "\`sweep undo\` cannot reach both concurrent applies. Both processes printed 'Undo with: sweep undo' as if that promise held for each of them independently, but sweep has no per-directory undo selector — it always reverses whichever journal has the newest mtime. After call 1, only $WHICH_FIRST was restored. After call 2: D1 restored=$D1_RESTORED2, D2 restored=$D2_RESTORED2. The directory left un-restored is not corrupted or lost, but its undo is silently unreachable through the documented CLI (\`sweep undo\` takes no PATH argument) — it sits fully-applied until the journal's 30-day TTL prunes it.
  call 1 said: $(cat "/tmp/conc_undo1.$$")
  call 2 said: $(cat "/tmp/conc_undo2.$$")"
fi

rm -f "/tmp/conc_d1_out.$$" "/tmp/conc_d2_out.$$" "/tmp/conc_undo1.$$" "/tmp/conc_undo2.$$"
