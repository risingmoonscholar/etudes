# Undo, status and journals

How sweep and stash record what they did, report it, and reverse it. Back to the [README](../README.md).

## Ask about an operation

To ask about an operation after the fact, without moving anything:

```sh
sweep status --id ID --json     # one sweep operation, by its id
sweep status PATH --json        # the operations recorded for a folder, each with its own undo step
stash status --id ID --json     # one stash operation, by its id
```

The id is `recovery.operation` in the result of the command that moved the
files. The answer gives a state (`refused`, `completed`, `interrupted`,
`reversed` or `partly_reversed`) and counts of planned, moved and reversed
items. `refused` is an apply that was turned away before it moved anything. It never
lists file names. An id that no journal holds gives exit 1 with the code
`unknown_operation`. `reversed` means nothing is still moved; `partly_reversed`
means some files are back and some are not.

In `sweep status`, `planned`, `moved` and `reversed` are states of each file: each
is a count of the files now in that state.

A result's `status` and `reason` always agree: when the status is `incomplete`,
`refused` or `error`, the reason is never `done` or `nothing_to_do`. An undo or
pop that had to leave a file behind exits 3, with status `incomplete` and the
code `operation_incomplete`. A failure that moved nothing has status `refused`.

When files are not where apply left them, `sweep undo` restores nothing for them
and says what to do. If you moved that folder, move it back and run the printed
command. If you deleted them, there is nothing to restore. `sweep undo` with no
folder keeps stopping at that record rather than reverse a folder you did not
name, so reach another folder's apply by naming it: `sweep undo FOLDER`, or one
apply by its id: `sweep undo --id ID`. In
`stash pop --latest` the same case prints the id of another stash, because `stash
pop --id ID` goes straight to one stash.

Two applies on one folder at once are not held apart. One wins. The other ends
in one of two ways, and timing decides which:

- **Refused before it moved anything** (exit 2, `plan_stale`): the folder
  changed under its plan. Nothing of its own needs undoing, and `sweep status`
  reports its record as `refused`. Scan and apply again.
- **Stopped part-way** (exit 3, `operation_incomplete`): it had started and
  then collided. `recovery.operation` is an id that `sweep status --id` reports
  as `interrupted`. Run `sweep undo` on the folder before planning again.

After the part-way ending, run `sweep undo` on the folder until it says there
is nothing left to undo. It reverses the apply that recorded the remaining
moves first, then the stopped apply, restoring every original path and byte.
This works even when the stopped apply's journal is the newest. Journals that
actually lost several move records are still refused without touching files.

Record order and a power cut. Each move's record is ordered behind its move with
a barrier sync, not a full drive flush, and a finished run is flushed once.
After a power cut, every surviving move except the newest has its record. The
newest move may lack one, or a record may survive without its move. `sweep undo`
and `stash pop` reconcile both.

You can undo an older apply before a newer one with `sweep undo --id ID`. It never
overwrites a file. If the newer apply moved those files, it restores none of them,
exits 3 and says to undo the newer one first; `sweep status FOLDER` lists each
apply with its own step. If both applies want the same place, the second to come
back stays where it is and says the place is taken. A folder the newer apply may
still need is kept; run the older step again later to remove it.

If `sweep undo` is killed after it has put the last file back, the folder that
`sweep apply` created can be left behind, empty. Run `sweep undo` again: it
removes that folder and finishes. It removes only a folder the journal says
apply created, and only if the folder is still the same one and has nothing
in it. A folder needed as an origin or guard by another live journal is kept;
unreadable history also keeps the folder. Undo reports how many empty folders
it retained for journals. A folder with anything inside it, or one that was
replaced, is left alone. Undo never deletes or overwrites a file.

## Where journals live

Journals are namespaced per tool and share `~/Library/Application
Support/etudes`, so `sweep undo` and `stash pop` cannot reverse each other's
work.

## Encrypted undo without a login keychain

Both `sweep` and `stash` accept `ETUDE_JOURNAL_KEY`: exactly 64 ASCII hex
characters encoding a randomly generated 256-bit key. When set, it is used
instead of the login keychain. An empty, invalid, or non-Unicode value is
refused; it never falls back to another key or to a plaintext journal.

For a temporary session, generate a key with the operating system's secure
random source, then keep the same environment for apply and restore:

```sh
export ETUDE_JOURNAL_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
sweep apply /path/to/folder --yes
sweep undo /path/to/folder
stash /path/to/another-folder --json
# Save details.stash_id from the receipt, then restore that exact batch:
stash pop --id ID
```

Keep a durable copy in your secret manager if you need undo after that shell
closes. Supply that same key through the environment on later runs. Do not
use a password, repeat a short value, or generate a fresh key before undo.
The tools validate the encoding and size; they cannot measure the randomness
of a supplied secret. Do not paste a literal key into command arguments or
shell history, print it in logs, or enable shell tracing while supplying it.
Child processes inherit environment variables, and code running as you may
be able to read them. Unset the variable when the session is finished.

The encrypted journal format and cipher are unchanged. `sweep forget` cannot
destroy a supplied key and refuses key destruction while the variable is set;
you must remove all retained copies yourself. Losing the key loses undo.
Use a separate `ETUDE_STATE_DIR` for each key if you maintain multiple keys.
Bare `sweep undo` and `sweep undo FOLDER` refuse to search past an unreadable newer
journal, even for a named folder. The refusal lists up to five records that do
open, newest first, each with its folder and its `sweep undo --id ID` step, and
says how many more exist. `sweep undo --id ID` opens only that record, so it also
reaches an older apply of the same folder. Do not delete a record that will not
open: under the right key it is still good, and sweep cannot tell a wrong key from
an altered record. Stash folder pop authenticates every stash journal before filtering by
folder. An unreadable older journal also blocks it, even if already completed
or from another folder: exit 3 names the undo record that cannot be opened and
restores nothing, with files and journal bytes unchanged. Without that record the
tool cannot say where its files are now, so it does not claim to. This also applies to `stash pop` in the
current directory. Restore a known batch with `stash pop --id ID` and its original
key; exact-ID selection does not search unrelated journals.

If key acquisition fails, the only way to move files without a journal is
explicit `--no-journal` on `sweep apply` or `stash`. This writes no journal
and removes undo; `stash pop` cannot restore an unjournaled stash. Files then
need to be restored manually.

Undo and pop also refuse when journal discovery or reads fail, when a journal
entry is not a regular file, or when journals have identical modification times
and their order cannot be established. They do not guess which operation is newer.
