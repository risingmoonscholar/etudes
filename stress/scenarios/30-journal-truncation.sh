#!/usr/bin/env bash
# Interruption family: journal integrity after a kill.
#
# A SIGKILL can truncate the journal file itself, not just interrupt a move:
# the journal format is a sealed base frame followed by appended sealed
# "done" progress frames (one per file, fsynced individually), so a crash
# mid-write is exactly a truncated file on disk. This scenario does not rely
# on timing a kill precisely. It takes a journal from a real, fully
# completed apply and truncates it at exact byte offsets after the fact,
# which is a strictly *easier* case than a real crash (a real crash can only
# ever cut off whole or partial trailing frames, same as this does).
#
# The property under test: "the tool refuses to act on a damaged journal
# rather than acting on a partial one." When this scenario was written,
# `apply_progress` half-replayed a truncated tail on purpose (as far as it
# verified and no further, silently), and this scenario is what caught that
# quietly producing stranded files under an exit code that reads as fine. That has
# since been fixed: a torn trailing frame now refuses the whole journal
# rather than being silently dropped. The scenario stays in place because the
# property is what matters, not the implementation that used to violate it.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

require stat "byte-exact journal truncation" || exit 0

make_tree() {
  local d="$1" n="$2"
  mkdir -p "$d"
  for i in $(seq 1 "$n"); do
    : > "$d/Screenshot 2026-0$((i % 9 + 1))-$(printf %02d $((i % 28 + 1))) at $(printf %02d $((i % 12 + 1))).$(printf %02d $((i % 60))).$(printf %02d $((i % 60))) AM ($i).png"
  done
}

journal_wipe() {
  # `sweep undo` always picks the journal with the newest mtime. With no
  # per-directory selector, stale journals from an earlier trial must be
  # cleared first or a truncation could land on the wrong file while undo
  # quietly acts on an untouched one, masking the very thing under test.
  rm -f "$ETUDE_STATE_DIR"/sweep-*.journal 2>/dev/null
}

journal_file() {
  find "$ETUDE_STATE_DIR" -maxdepth 1 -name 'sweep-*.journal' 2>/dev/null | head -1
}

# --- Build one fully-applied tree and snapshot its intact journal ----------
W=$(workdir) || exit 1; trap 'rm -rf "$W"' EXIT
D="$W/D"
N=60
make_tree "$D" "$N"
BEFORE_SET=$(find "$D" -maxdepth 1 -type f -exec basename {} \; | sort)
BEFORE_N=$(echo "$BEFORE_SET" | grep -c .)

journal_wipe
assert_exit 0 "truncation setup apply succeeds" -- "$SWEEP" apply "$D" --yes
JF=$(journal_file)
if [ -z "$JF" ] || [ ! -f "$JF" ]; then
  fail "no journal file found after successful apply"
  exit 0
fi
FULL_SIZE=$(stat -f%z "$JF" 2>/dev/null || stat -c%s "$JF")
BACKUP="$(dirname "$D")/journal-backup.bin"
cp "$JF" "$BACKUP"

# --- Shallow truncations: within/at the base frame. Must be refused. -------
SHALLOW_BAD=0
SHALLOW_FIRST=""
for off in 0 1 4 8 50; do
  [ "$off" -ge "$FULL_SIZE" ] && continue
  head -c "$off" "$BACKUP" > "$JF" 2>/dev/null
  snapshot_tree "$D" "$(dirname "$D")/shallow-before.json"
  out=$("$SWEEP" undo 2>&1)
  code=$?
  snapshot_tree "$D" "$(dirname "$D")/shallow-after.json"
  assert_snapshot_eq "$(dirname "$D")/shallow-before.json" "$(dirname "$D")/shallow-after.json" \
    "shallow cut at $off bytes leaves the exact tree untouched"
  assert_intact "$D" "$BEFORE_N" "shallow cut at $off bytes preserves every file"
  # A shallow cut must be REFUSED outright: nonzero exit, and it must not
  # have silently "succeeded" its way into moving anything.
  assert_eq 3 "$code" "shallow cut at $off bytes reports a damaged journal"
  assert_no_panic "shallow cut at $off" "$out"
  if [ "$code" != "3" ]; then
    SHALLOW_BAD=$((SHALLOW_BAD + 1))
    [ -z "$SHALLOW_FIRST" ] && SHALLOW_FIRST="offset=$off/$FULL_SIZE: exit 0 instead of a refusal. Output: $out"
  fi
done

if [ "$SHALLOW_BAD" -eq 0 ]; then
  pass "journal truncated within its sealed base frame is refused outright (exit 3), never acted on"
else
  fail "journal truncated within its sealed base frame was NOT refused in $SHALLOW_BAD case(s). First: $SHALLOW_FIRST"
fi

# --- Deep truncations: base frame intact, some trailing progress frames cut.
# This is precisely what a SIGKILL leaves behind mid-apply on a large batch:
# the base is written once up front and is complete; only the *tail* of
# per-file progress frames is at risk of being cut short.
DEEP_BAD=0
DEEP_FIRST=""
for pct in base 60 75 85 95 99; do
  # Reset to a clean fully-applied, fully-intact-journal state each time.
  # journal_wipe first: a stale (possibly still-truncated) journal from the
  # shallow-truncation pass or a prior loop iteration must not linger and be
  # picked up as "the newest" by mistake.
  rm -rf "$D"; make_tree "$D" "$N"
  journal_wipe
  assert_exit 0 "truncation setup apply succeeds" -- "$SWEEP" apply "$D" --yes
  JF=$(journal_file)
  [ -n "$JF" ] && [ -f "$JF" ] || { fail "no journal after deep-cut setup apply"; exit 0; }
  cp "$JF" "$BACKUP"
  full=$(stat -f%z "$BACKUP" 2>/dev/null || stat -c%s "$BACKUP")
  # `base` means "cut exactly at the end of the base frame": every progress
  # record gone, on a clean boundary. Added because the percentage cuts all
  # land mid-frame, which leaves a torn tail the loader can notice -- and the
  # case that actually shipped a lie was the one it CANNOT notice. With every
  # record gone and no tear, each entry reads Planned and the journal looks
  # exactly like an apply that never moved anything, while every file is at
  # its destination. Only the filesystem can tell those apart.
  if [ "$pct" = "base" ]; then
    cutoff=$(( 4 + $(python3 -c "
import struct,sys
raw=open(sys.argv[1],'rb').read()
print(struct.unpack('<I', raw[0:4])[0])" "$BACKUP") ))
  else
    cutoff=$(( full * pct / 100 ))
  fi
  head -c "$cutoff" "$BACKUP" > "$JF"

  snapshot_tree "$D" "$(dirname "$D")/deep-before.json"
  out=$("$SWEEP" undo 2>&1)
  code=$?
  after_set=$(find "$D" -maxdepth 1 -type f -exec basename {} \; | sort)
  stranded=$(find "$D" -mindepth 2 -type f 2>/dev/null | wc -l | tr -d ' ')
  assert_intact "$D" "$BEFORE_N" "deep cut at $pct preserves every file"
  if [ "$code" != "0" ] && [ "$code" != "1" ]; then
    snapshot_tree "$D" "$(dirname "$D")/deep-after.json"
    assert_snapshot_eq "$(dirname "$D")/deep-before.json" "$(dirname "$D")/deep-after.json" \
      "deep cut at $pct refuses without changing the tree"
  else
    assert_eq "$BEFORE_SET" "$after_set" "deep cut at $pct restores every original top-level name"
  fi

  # Two acceptable outcomes for a damaged journal: (a) refuse outright
  # (nonzero exit, nothing touched), or (b) fully succeed with every file
  # actually restored.
  #
  # What must NOT happen is a reassuring exit while files are still stranded.
  # That means exit 0 AND exit 1. This used to test exit 0 only, and the gap
  # was not theoretical: a journal cut back to its base frame made undo report
  # "Nothing to undo. The most recent recorded apply was already restored" and
  # exit 1, with every file at its destination. Exit 1 means "nothing to do",
  # which is the same lie as "success" told in a quieter voice, and this
  # scenario passed the whole time.
  #
  # Exit 3 with files stranded is fine: that is a refusal saying so.
  if { [ "$code" = "0" ] || [ "$code" = "1" ]; } && \
      { [ "$stranded" != "0" ] || [ "$after_set" != "$BEFORE_SET" ]; }; then
    DEEP_BAD=$((DEEP_BAD + 1))
    if [ -z "$DEEP_FIRST" ]; then
      DEEP_FIRST="offset=${pct}% ($cutoff/$full bytes): exit $code but the original top-level names were not fully restored; $stranded file(s) remain at their destination.
  sweep undo said: $out
  origin has $(echo "$after_set" | grep -c .)/$BEFORE_N files; stranded: $(find "$D" -mindepth 2 -type f -exec basename {} \; 2>/dev/null | tr '\n' ',')"
    fi
  fi
done

if [ "$DEEP_BAD" -eq 0 ]; then
  pass "journal truncated past its base frame (trailing progress cut, as a real crash would leave it) is either refused or fully honoured. Never a silent partial success"
else
  fail "journal truncated past its base frame exits 0 or 1 without restoring every original top-level name in $DEEP_BAD/6 cases. First reproduction:
$DEEP_FIRST"
fi

rm -rf "$(dirname "$D")" "$BACKUP" 2>/dev/null
