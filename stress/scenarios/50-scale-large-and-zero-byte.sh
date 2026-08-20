#!/usr/bin/env bash
# Zero-byte files, and one large (sparse) file. Two claims to check directly
# rather than take on faith:
#
#   - "moved rather than copied where possible" (apply.rs move_one: tries
#     fs::hard_link + unlink before ever falling back to a real copy, and
#     hard_link is a directory-entry operation: O(1) in the file's size).
#     A hard link preserves the inode number; a copy creates a new one. That
#     is a hard, checkable fact, not a timing guess.
#   - "nothing reads its contents" (journal.rs edge_hash reads at most 4 KiB
#     from the start and 4 KiB from the end, regardless of file size). A
#     multi-hundred-MB sparse file should fingerprint and move in about the
#     same wall-clock time as a handful of empty files, not time proportional
#     to its size.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

W=$(workdir); trap 'rm -rf "$W"' EXIT
D="$W/tree"; mkdir -p "$D"

# --- zero-byte files, a real group of them ---------------------------
for i in 1 2 3 4 5; do : > "$D/empty_${i}_zerotok.txt"; done

# --- one large sparse file, grouped with small siblings ------------------
BIGFILE="$D/huge_recording.mp4"
SIZE_MB=500
if ! truncate -s "${SIZE_MB}M" "$BIGFILE" 2>/dev/null; then
  unproven "large sparse file is moved (not copied) and its content is never read" "\`truncate\` is not available on this host to build a sparse file"
else
  # Siblings in the SAME family as the big file. They were .txt, which groups
  # as Documents while the .mp4 groups as Media, so the big file would have
  # been alone in its family and no group would have formed.
  for i in 1 2 3 4; do : > "$D/small_${i}_clip.mp4"; done

  APPARENT_SIZE=$(stat -f%z "$BIGFILE")
  assert_eq "$((SIZE_MB * 1024 * 1024))" "$APPARENT_SIZE" "the sparse file reports the full $SIZE_MB MB apparent size"

  DISK_BLOCKS_BEFORE=$(du -k "$BIGFILE" | cut -f1)
  printf '    sparse file: %s MB apparent, %s KB actually on disk\n' "$SIZE_MB" "$DISK_BLOCKS_BEFORE" >&2
  if [ "$DISK_BLOCKS_BEFORE" -lt $((SIZE_MB * 1024 / 2)) ]; then
    pass "the fixture file is genuinely sparse (disk usage far below apparent size). A real test of 'don't read it', not an accident of a fully-written file"
  else
    unproven "fixture sparseness" "truncate did not produce a sparse file on this filesystem (disk usage ≈ apparent size); the move-not-copy and no-read checks below still run, but a real copy would be harder to distinguish from a move by disk usage alone"
  fi

  INODE_BEFORE=$(stat -f%i "$BIGFILE")

  BEFORE=$(find "$D" -type f | wc -l | tr -d ' ')
  assert_eq 10 "$BEFORE" "fixture has 5 zero-byte files + 1 large file + 4 small siblings = 10 files"

  # Plan first: this is where edge_hash touches every member, including the
  # huge file, to fingerprint it for the journal. Fingerprinting happens at
  # apply time (apply.rs calls fingerprint() while building journal entries),
  # not at plan time. The real test is apply's wall clock.
  t0=$(date +%s.%N)
  APPLY_OUT=$("$SWEEP" apply "$D" --yes 2>&1)
  APPLY_EC=$?
  t1=$(date +%s.%N)
  APPLY_S=$(echo "$t1-$t0"|bc)
  printf '    apply (includes fingerprinting + moving the %s MB file): %ss\n' "$SIZE_MB" "$APPLY_S" >&2
  assert_eq 0 "$APPLY_EC" "apply succeeds on a tree containing a $SIZE_MB MB file and five 0-byte files"

  NEWPATH=$(find "$D" -name "huge_recording.mp4" 2>/dev/null | head -1)
  # It must have MOVED, not merely still exist. The fixture used a .bin
  # extension that is in no type family, so no group formed, and every check
  # below was measuring a file that had never been touched -- the inode was
  # unchanged because nothing moved it.
  if [ -z "$NEWPATH" ]; then
    fail "the large file cannot be found anywhere after apply"
  elif [ "$NEWPATH" = "$BIGFILE" ]; then
    fail "the large file is still at its original path. No group formed, so the move this scenario measures never happened and the inode and size checks below would prove nothing"
  else
    INODE_AFTER=$(stat -f%i "$NEWPATH")
    if [ "$INODE_BEFORE" = "$INODE_AFTER" ]; then
      pass "the $SIZE_MB MB file kept its inode across the move ($INODE_BEFORE). Proof it was moved (hard-link+unlink), not copied"
    else
      fail "the $SIZE_MB MB file's inode changed across apply (before=$INODE_BEFORE after=$INODE_AFTER). This means it was actually COPIED, not moved. For a file this size that is a real performance and disk-space regression, not just a style issue."
    fi

    SIZE_AFTER=$(stat -f%z "$NEWPATH")
    assert_eq "$APPARENT_SIZE" "$SIZE_AFTER" "the moved file's apparent size is unchanged"

    DISK_BLOCKS_AFTER=$(du -k "$NEWPATH" | cut -f1)
    assert_eq "$DISK_BLOCKS_BEFORE" "$DISK_BLOCKS_AFTER" "actual disk usage is unchanged by the move (no real data was written. A copy of a sparse file this size would either materialize the holes or at best re-punch them, and would not be free)"
  fi

  # The real evidence for "nothing reads its contents": apply's wall-clock
  # cost here should be governed by fsync-per-move overhead (see
  # 50-scale-flat-10k-apply-timing.sh), not by 500 MB of I/O. A few hundred
  # MB read at even a slow 200 MB/s would add ~2.5s; reading it at typical
  # SSD speeds would be closer to instant anyway. This bound is
  # deliberately loose. It is there to catch a full read/copy, not to
  # re-litigate storage speed.
  if (( $(echo "$APPLY_S < 15" | bc -l) )); then
    pass "apply on a tree containing a $SIZE_MB MB file completed in ${APPLY_S}s. Consistent with fingerprinting only the first/last 4 KiB (edge_hash), not the whole file"
  else
    fail "apply took ${APPLY_S}s for 10 files including one $SIZE_MB MB file. Slow enough to suggest the whole file's content was read or copied rather than moved"
  fi

  # --- undo: same inode-preservation check in reverse ---------------------
  "$SWEEP" undo >/dev/null 2>&1
  UNDO_EC=$?
  assert_eq 0 "$UNDO_EC" "undo succeeds"
  AFTER_UNDO=$(find "$D" -maxdepth 1 -type f | wc -l | tr -d ' ')
  assert_eq "$BEFORE" "$AFTER_UNDO" "undo restored all 10 files to the top level"
  if [ -f "$BIGFILE" ]; then
    INODE_RESTORED=$(stat -f%i "$BIGFILE")
    assert_eq "$INODE_BEFORE" "$INODE_RESTORED" "undo moved the large file back via the same inode (moved, not re-copied, in reverse too)"
  else
    fail "the large file did not come back to its original path after undo"
  fi
fi

# --- zero-byte files specifically: no divide-by-zero, no special-case bug -
D2="$W/zeros"; mkdir -p "$D2"
for i in 1 2 3 4 5; do : > "$D2/z_${i}_zonlytok.txt"; done
BEFORE_Z=$(find "$D2" -type f | wc -l | tr -d ' ')
assert_exit 0 "planning an all-zero-byte-file directory succeeds (no crash on size=0 in edge_hash)" -- "$SWEEP" "$D2"
"$SWEEP" apply "$D2" --yes >/dev/null 2>&1
APPLY_Z_EC=$?
assert_eq 0 "$APPLY_Z_EC" "applying an all-zero-byte-file group succeeds"
AFTER_Z=$(find "$D2" -type f | wc -l | tr -d ' ')
assert_eq "$BEFORE_Z" "$AFTER_Z" "no zero-byte file was lost"
"$SWEEP" undo >/dev/null 2>&1
RESTORED_Z=$(find "$D2" -maxdepth 1 -type f | wc -l | tr -d ' ')
assert_eq "$BEFORE_Z" "$RESTORED_Z" "undo restores zero-byte files correctly (their edge_hash of an empty head/tail is a valid, stable fingerprint)"
