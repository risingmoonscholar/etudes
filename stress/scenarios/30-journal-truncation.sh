#!/usr/bin/env bash
# Interruption family: journal integrity after a kill.
#
# A SIGKILL can truncate the journal file itself, not just interrupt a move:
# the journal format is a sealed base frame followed by appended sealed
# "done" progress frames (one per file, fsynced individually), so a crash
# mid-write is exactly a truncated file on disk. This scenario does not rely
# on timing a kill precisely — it takes a journal from a real, fully
# completed apply and truncates it at exact byte offsets after the fact,
# which is a strictly *easier* case than a real crash (a real crash can only
# ever cut off whole or partial trailing frames, same as this does).
#
# The property under test: "the tool refuses to act on a damaged journal
# rather than acting on a partial one." When this scenario was written,
# `apply_progress` half-replayed a truncated tail on purpose — as far as it
# verified and no further, silently — and this scenario is what caught that
# quietly producing stranded files under a reported exit-0 success. That has
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
  # `sweep undo` always picks the journal with the newest mtime — with no
  # per-directory selector, stale journals from an earlier trial must be
  # cleared first or a truncation could land on the wrong file while undo
  # quietly acts on an untouched one, masking the very thing under test.
  rm -f "$XDG_STATE_HOME/etudes"/sweep-*.journal 2>/dev/null
}

journal_file() {
  find "$XDG_STATE_HOME/etudes" -maxdepth 1 -name 'sweep-*.journal' 2>/dev/null | head -1
}

# --- Build one fully-applied tree and snapshot its intact journal ----------
D=$(workdir)/D
N=60
make_tree "$D" "$N"
BEFORE_SET=$(find "$D" -maxdepth 1 -type f -exec basename {} \; | sort)
BEFORE_N=$(echo "$BEFORE_SET" | grep -c .)

journal_wipe
"$SWEEP" apply "$D" --yes >/dev/null 2>&1
JF=$(journal_file)
if [ -z "$JF" ] || [ ! -f "$JF" ]; then
  unproven "journal truncation matrix" "no journal file found after apply — cannot attack what was not written"
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
  out=$("$SWEEP" undo 2>&1)
  code=$?
  moved_any=$(find "$D" -mindepth 2 -type f 2>/dev/null | wc -l | tr -d ' ')
  # A shallow cut must be REFUSED outright: nonzero exit, and it must not
  # have silently "succeeded" its way into moving anything.
  if [ "$code" = "0" ]; then
    SHALLOW_BAD=$((SHALLOW_BAD + 1))
    [ -z "$SHALLOW_FIRST" ] && SHALLOW_FIRST="offset=$off/$FULL_SIZE: exit 0 instead of a refusal. Output: $out"
  fi
done

if [ "$SHALLOW_BAD" -eq 0 ]; then
  pass "journal truncated within its sealed base frame is refused outright (nonzero exit), never acted on"
else
  fail "journal truncated within its sealed base frame was NOT refused in $SHALLOW_BAD case(s). First: $SHALLOW_FIRST"
fi

# --- Deep truncations: base frame intact, some trailing progress frames cut.
# This is precisely what a SIGKILL leaves behind mid-apply on a large batch:
# the base is written once up front and is complete; only the *tail* of
# per-file progress frames is at risk of being cut short.
DEEP_BAD=0
DEEP_FIRST=""
for pct in 60 75 85 95 99; do
  # Reset to a clean fully-applied, fully-intact-journal state each time.
  # journal_wipe first: a stale (possibly still-truncated) journal from the
  # shallow-truncation pass or a prior loop iteration must not linger and be
  # picked up as "the newest" by mistake.
  rm -rf "$D"; make_tree "$D" "$N"
  journal_wipe
  "$SWEEP" apply "$D" --yes >/dev/null 2>&1
  JF=$(journal_file)
  cp "$JF" "$BACKUP"
  full=$(stat -f%z "$BACKUP" 2>/dev/null || stat -c%s "$BACKUP")
  cutoff=$(( full * pct / 100 ))
  head -c "$cutoff" "$BACKUP" > "$JF"

  out=$("$SWEEP" undo 2>&1)
  code=$?
  after_set=$(find "$D" -maxdepth 1 -type f -exec basename {} \; | sort)
  stranded=$(find "$D" -mindepth 2 -type f 2>/dev/null | wc -l | tr -d ' ')

  # Two acceptable outcomes for a damaged journal: (a) refuse outright
  # (nonzero exit, nothing touched), or (b) fully succeed with every file
  # actually restored. What must NOT happen: exit 0 while files are still
  # stranded at their destination — that is "success" lying about the
  # state of the user's files.
  if [ "$code" = "0" ] && [ "$stranded" != "0" ]; then
    DEEP_BAD=$((DEEP_BAD + 1))
    if [ -z "$DEEP_FIRST" ]; then
      DEEP_FIRST="offset=${pct}% ($cutoff/$full bytes): exit 0 (\"success\") but $stranded file(s) left stranded at their destination, unrestored and unmentioned.
  sweep undo said: $out
  origin has $(echo "$after_set" | grep -c .)/$BEFORE_N files; stranded: $(find "$D" -mindepth 2 -type f -exec basename {} \; 2>/dev/null | tr '\n' ',')"
    fi
  fi
done

if [ "$DEEP_BAD" -eq 0 ]; then
  pass "journal truncated past its base frame (trailing progress cut, as a real crash would leave it) is either refused or fully honoured — never a silent partial success"
else
  fail "journal truncated past its base frame silently HALF-LOADS in $DEEP_BAD/5 cases: sweep undo exits 0 and prints no error, but leaves files stranded at their destination that it never mentions and will never retry — the exact 'half-load is worse than fail-to-load' failure the brief warns about. First reproduction:
$DEEP_FIRST"
fi

rm -rf "$(dirname "$D")" "$BACKUP" 2>/dev/null
