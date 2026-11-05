# Unpack staging recovery

Measured on 2026-11-03, macOS 27.0.1, build 26A434. The pre-change commit
was `e410c5c`. No desktop application was driven.

## Apple documentation and replacement-directory measurements

Read with the `macos-docs` skill's `apple-doc` command:

- [FileManager.SearchPathDirectory.itemReplacementDirectory](https://developer.apple.com/documentation/foundation/filemanager/searchpathdirectory/itemreplacementdirectory)
  describes the temporary-directory constant.
- [FileManager.url(for:in:appropriateFor:create:)](https://developer.apple.com/documentation/foundation/filemanager/url%28for:in:appropriatefor:create:%29)
  says that the supplied URL selects the volume, warns against relying on
  the directory after app exit, and recommends removing it when no longer needed.

For a documented promise that macOS removes these directories after a killed
process: **no doc found**. The two pages above and searches of Apple's
documentation for `itemReplacementDirectory` with killed-process and SIGKILL
cleanup terms did not provide one. No claim about automatic removal is made.

The tiny Swift probe was written under `$TMPDIR/etudes-staging-measure`:

```swift
import Foundation
let manager = FileManager.default
let source = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
let folder = try manager.url(for: .itemReplacementDirectory,
    in: .userDomainMask, appropriateFor: source, create: true)
print("source=\(source.path)")
print("replacement=\(folder.path)")
try manager.removeItem(at: folder)
```

It ran with `DEVELOPER_DIR=/Library/Developer/CommandLineTools`, using a
module cache under the same temporary directory. Observed locations:

| Supplied volume | Returned location |
| --- | --- |
| Boot volume, sandboxed command | `$TMPDIR/(A Document Being Saved By swift-frontend)` |
| Boot volume, command with sandbox restrictions lifted | `$TMPDIR/TemporaryItems/NSIRD_swift-frontend_nbtfTG` |
| Private APFS image, command with sandbox restrictions lifted | `<mount>/.TemporaryItems/folders.501/TemporaryItems/NSIRD_swift-frontend_aIwct7` |

The APFS mount was
`/private/var/folders/1z/r3hvhpx17vx5kvs_jh7tg8700000gn/T/etudes-staging-measure/apfs-mount`.
The worktree and `$TMPDIR` reported the same device number, 16777233.
The APFS image was 64 MB, mounted with `hdiutil attach -nobrowse -mountpoint`
under `$TMPDIR`, then detached successfully (exit 0). The probe removed its
own replacement directory before detaching.

Sandboxed APFS and exFAT image creation both failed with `Device not
configured`. With restrictions lifted, APFS creation and mounting succeeded.
exFAT remained unmeasured: `hdiutil create -fs ExFAT` exited 1 with
`Operation not permitted`; the locally documented `diskutil image create blank
-size 64m -fs ExFAT --volumeName ...` also exited 1 with
`LOCALIZED_STRING_UNAVAILABLE`. No exFAT image was mounted.

These measurements do not establish a better location with a documented
SIGKILL cleanup guarantee. Staging remains beside the destination on its volume.

## Advisory locks

Read local `man 2 flock` and `man 2 close` before implementation.
`flock(2)` defines nonblocking exclusive locks and shared references across
fork. `close(2)` says the last close releases an advisory lock and that exit
frees the process's descriptors. SDK `sys/fcntl.h` supplied `O_NOFOLLOW`,
`O_NONBLOCK` and `F_SETFD`; `close(2)` documents clearing close-on-exec.

A small Python probe forked a holder, used a pipe to witness its acquired
exclusive lock, and attempted a second `LOCK_EX | LOCK_NB`. It then sent real
SIGKILL, waited for the holder's signal exit and tried the second lock again.

| Volume | While holder lived | After SIGKILL and wait |
| --- | --- | --- |
| Boot volume | Second lock refused | Second lock acquired |
| Private APFS image | Second lock refused | Second lock acquired |
| exFAT | Unmeasured | Unmeasured |

The extractor inherits the open lock descriptor. Closing the supervisor's
reference does not explicitly unlock the surviving writer's reference.
The stress scenario verifies this with the actual ZIP extractor, stopped
while part of the archive exists, after killing its supervisor.

## Recovery protocol and limits

Only candidates for the same destination name under the same parent are
considered. Removal requires all of:

- Exact `.{name}.unpack-{32 lowercase hex digits}.partial` name.
- `symlink_metadata` proving a real directory owned by the effective user.
- A real, singly linked lock file owned by that user, opened without following
  a symlink and without blocking on a substituted FIFO.
- The complete `unpack staging lock v1` marker naming this directory's device
  and inode, written only after the creator already holds the exclusive lock.
  A copy of a leftover has another inode, so it proves nothing and stays.
- A nonblocking exclusive lock, followed by directory and lock identity checks.

The initialized marker closes the race between creating the lock file and
taking its first lock. A second cleaner that opened a subsequently removed
lock cannot use the unlinked descriptor to authorize deletion at its old path.
Lock names include the staging nonce; reservation retries if any archive path
component would collide, including after wrapper flattening.

The lock stays open across extraction, tidying, quarantine preservation and
the final audit. Its file is excluded from payload byte measurements and
quarantine writes, and removed immediately before publication. A kill in that
final unlink/rename interval leaves a folder without a verifiable lock; it is
retained and reported, like older staging. A kill before lock initialization
also leaves an unprovable candidate. These conservative limits are deliberate.

**SIGKILL itself still leaves staging until a later extraction into the same
destination under the same parent.** A live inherited writer postpones recovery
until its descriptor is released. Lockless, linked, unreadable, malformed,
copied or otherwise unverified folders with the exact name remain and are
reported by path. A name that only resembles a leftover is neither touched nor
mentioned. Recursive removal failures are reported without blocking the new
extraction or claiming that cleanup succeeded. A run that finds a folder another
run has just removed says nothing about it. Under a read-only parent the folder cannot lose its
name, so recovery does not start: the contents stay, and the printed step makes
the parent writable first. A destination name keeps its exact bytes in the folder
name, so names that read the same when made lossy stay separate destinations. No process is killed by recovery.

A volume that answers `flock` with ENOTSUP, EOPNOTSUPP or ENOLCK still extracts,
as it did before locks existed. Its folder has no lock file, so a later run
cannot prove it abandoned and leaves it in place. No such volume was mounted for
this: a unit test injects the error at the lock call.

The reusable advisory guard lives in `etude-cli-support`; staging policy and
recovery are unpack-specific. Pack is unchanged. Unpack declaration version 8
adds recovery reads and deletion of verified abandoned staging; version 7 is
retained. The result-envelope and progress schemas are unchanged.

## Regression witnesses

`stress/scenarios/114-unpack-staging-recovery.sh` makes a 5,000-member stored
ZIP, witnesses partial extraction, stops the supervisor and extractor, and
retries while each holds the lock. It uses real SIGKILL and later checks every
published path and byte. All generated subprocess groups are terminated on
exit; polling has deadlines and sleeps.

It also plants a matching legacy directory without a lock, a matching
symlink, a similar ordinary file, an exactly matching ordinary file, an
uninitialized marker, a symlink lock, a FIFO lock, an unreadable lock, a
directory whose contents cannot be removed and staging for a different
destination. Two simultaneous retries must publish exactly once and preserve
the complete result. A unit test covers the empty-lock initialization window.

| Binary | Scenario exit | Passed | Failed |
| --- | --- | --- | --- |
| Pre-change `e410c5c`, built from `git archive HEAD` in a private temporary copy | 1 | 13 | 13 |
| New code with only `staging::recover(&dest)` disabled in that temporary copy | 1 | 13 | 13 |
| Recovery enabled | 0 | 26 | 0 |

Both negative controls fail the killed-staging removal and competing-recovery
assertions, as well as the required retained-candidate path diagnostics. No
scenario or assertion was weakened. The final measured test count is 666 and
the tracked scenario count was 65 (46 fast, 9 load, 10 platform) at that commit.

Scenario 97 killed unpack
after seeing staging but before its writer could be reliably enumerated, then
raced that writer during fixture removal. Its start witness now requires a
real fixture member and freezes the supervisor before enumerating children.
It also requires the inherited lock to become available after killing the
writer, before removing the fixture. All existing assertions remain, with
three additional shutdown assertions. A direct run passed all 19 assertions.

The load-tier interrupted-extraction scenario's timing helpers now count its
`chunk_*.bin` payload rather than the internal lock file. This makes both its
partial-progress and completion witnesses refer to archived payload, with all
existing assertions retained. That load scenario was not run.
