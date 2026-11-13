# unpack

Unpack opens an archive without the surprises. Back to the [README](../README.md).

`unpack` has no undo. It writes no journal, so no command reverses an
extraction.

## An audited transaction

**Unpack publishes an audited transaction.** Extraction happens in a private
0700 staging directory beside the destination. Before tidying and again before
the final rename, a tree audit checks every actual member, including implicit
directories, for containment and type. Only directories and ordinary files
are accepted; links, special files, setuid/setgid modes and unreadable members
refuse publication. The requested destination appears with one rename after
the final audit. Failed runs remove staging; if removal fails, the diagnostic
names the remaining staging path or explicitly says its state is unverified.
Unpack holds the staging folder open while it works. If the destination's
parent is moved during extraction, the staging folder moves with it; unpack
finds it where it went, checks by device and inode that it is the same folder,
and removes it there. A folder that took the old name is not touched. If unpack
cannot find the staging folder again, it does not say nothing was left: it
names the old place and the hidden folder to look for.

SIGKILL itself still leaves a private folder until a later extraction into the same
destination under the same parent. That run removes the folder only when its name is exactly
`.{name}.unpack-{32 lowercase hex digits}.partial`, it is a real directory
owned by this user, its lock file names that very directory by device and inode,
and that lock can be taken. The extractor inherits the lock, so a surviving writer
also prevents removal. A symlink, a copy, a folder from an older version, a live
extraction and anything else unproved stay in place. Stderr names each folder
removed and each folder with that exact name left in place, and `--json` counts
them under `effects`. A failed removal is reported and does not block the new
extraction. On a volume without advisory locks, extraction works as before and its
leftover is never removed. See [staging recovery evidence](release-evidence/unpack-staging-recovery.md).

**Unpack's size budget is a monitored soft limit.** `--max-size N[G|M]` replaces
the default of half the target volume's free space. System extractors write
directly, so unpack measures logical file bytes every 100 ms and after exit,
then kills and reaps an over-budget extractor and measures again. Refusals
report the final exact byte total, limit and overshoot, with the code
`size_limit_exceeded`. `--max-size 0` is refused: the limit must be a whole number
above 0. Options may come before or after the archive. A relative path gets the
same free-space floor and limit as an absolute one. Read errors stop the
transaction instead of silently undercounting. This is not a hard write cap,
and filesystem allocation, metadata and the private archive copy are outside
that logical-byte measurement.

On 2026-10-06, macOS 27.0.1 arm64, the maximum measured overshoot in
`80-finder-unpack` was **16,777,215 bytes**: a 16,777,216-byte payload against a
one-byte limit. The scenario runs ZIP, tar and gzip with one-byte and 1 MiB
limits, three repetitions each (18 trials), and prints
`MEASURED_MAX_OVERSHOOT_BYTES`. Reproduce with `bash stress/run.sh 80-finder-unpack`.
This maximum describes that fixture on that host; larger or faster extractions
can overshoot further. There is no guaranteed maximum overshoot.

## Download quarantine on extracted archives

Unpack captures `com.apple.quarantine` from the same no-follow source descriptor
used to copy the archive. A change to that attribute during copying refuses the
operation. After junk removal and wrapper flattening, unpack applies the captured
opaque value to every final file and directory, including the destination root,
and reads it back before publishing. Attribute values are never printed. A source
without that attribute does not cause unpack to add one.

On macOS 27.0.1 (26A434), synthetic raw-extractor probes observed `/usr/bin/unzip
-o -q` and `/usr/bin/tar -xf/-xzf/-xjf/-xJf` propagating the archive mark; repeated
observations can include a changed mark rather than exact equality. `/usr/bin/gunzip
-c` with stdout redirected to a file omitted it. The unmodified unpack binary
anchored by copying the archive: ZIP/JAR and all TAR suffixes produced changed
payload marks and no marked destination root; bare GZIP omitted the mark entirely.
The complete extraction path is measured separately from the raw extractor.
`crates/unpack-cli/tests/quarantine_matrix.py` probes all ten supported suffixes,
with marked and unmarked sources, and checks final payload bytes and attributes.

macOS can represent attributes as `._` AppleDouble companions on exFAT. These
companions are retained when needed to store the final quarantine state. Unpack
refuses publication and removes staging if the captured attribute cannot be set
or read back exactly; it never reports a successful downgrade to unmarked output.
A native extended-attribute filesystem is not required when the OS's companion
representation supports exact readback. The quarantine receipt reports read
attempts and verified comparisons, without recording the attribute value.

Sources: Apple's [getxattr(2)](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/getxattr.2.html)
and [fsetxattr(2)](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsetxattr.2.html)
document descriptor access, absent attributes, and unsupported storage. Extractor
propagation and exFAT storage are measured behavior, rather than promises inferred
from these manuals. The unpack capability declaration is version 9 and the result
envelope is version 4. `docs/contracts/v9/unpack.json` is the canonical pin and
`v1/unpack.json` mirrors it; earlier pins stay immutable.
