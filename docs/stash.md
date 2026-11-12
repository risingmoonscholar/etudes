# stash

Stash moves files into a hidden holding folder in the same folder, without deleting them. Nothing happens on its own; `stash pop` moves them back. Back to the [README](../README.md).

To save a plan and apply exactly that plan later, see [Apply the plan you exported](sweep.md#rules-the-tools-share). For restoring batches, see [independent stash batches](stash-batches.md).

## Stash exactly the selected objects

`stash select PATH...` takes the chosen files and directories from any number of
parents. A selected directory moves as one opaque object; its children are not
scanned. The existing `stash DIR` command still stashes that directory's visible
contents. Explicit selections can include hidden files, but retain the existing
system and credential-directory location refusals. Known sync locations matched
by the existing path-marker rules are refused for selected sources and holding
parents before any move. This mode has no cloud-support override. Explicit symlinks and special
files are refused before holding storage is created. Directory selections require
holding storage on their own volume; regular files retain the existing copy
fallback when crossing volumes.

`stash select --from0 FILE` reads UTF-8 paths terminated by NUL; use `--from0 -`
for stdin. Newlines inside a path are preserved. The list must terminate every
path, contains no empty records, and is limited to 32 MiB and 100,000 objects.
Positional sources can accompany a list. Use `--` before option-shaped positional
filenames. Duplicate canonical paths, repeated object identities such as hard-link
aliases, and any ancestor/descendant overlap are refused before any move. Overlap
checks sort canonical path components; they do not scan a common ancestor.

Selections require an encrypted journal. `--no-journal` remains available for
legacy whole-folder stashing and is refused for explicit selections. Each selection
uses one private operation root and numbered holding slots, so equal basenames
from different parents cannot collide. `--into FOLDER` chooses where they are held;
otherwise the first canonical selected object's parent is used. A folder inside a
selected directory is refused, and one that does not exist gives `no folder at
PATH`, exit 3.

The result prints counts and a persistent operation id; it does not list source
paths, basenames, or the holding path. JSON carries `details.stash_id`; selection
still includes `details.holding_root: "<redacted>"`, not a usable path.
Restore the whole operation with `stash pop --id ID`. Use `stash inspect --id ID` to see path-free opaque item
ids and which items remain held, then `stash pop --id ID --item ITEM` to restore
one item. Each selected restore is durably recorded, and inspection reports the
actual held count after partial restores. Where the
state folder's volume supports advisory locks, one pop of a stash runs at a
time: a second pop of the same stash says another pop is restoring it, waits
for that pop to finish, then restores only what is still held. Where it does
not, two pops of one stash are not kept apart. Pop uses the original absolute parents
stored in the encrypted journal, and removes only empty holding directories
authenticated by that journal. Selected directories retain the existing opaque
directory fingerprint behavior; this is not a recursive content-integrity
claim. Receipts continue to name uninstrumented access as unproven, and no zero
counter proves absence of reads.

Journalled stash operations return a persistent `stash_id`. Use `stash pop --id ID` for exact restoration, or `stash pop --latest` to explicitly select the latest live batch. Folder operations can coexist; ambiguous path-based restores refuse. Deadlines remain information until an explicit pop. See [independent stash batches](stash-batches.md) for identity, legacy compatibility and disclosure semantics.
