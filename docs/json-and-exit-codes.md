# JSON, exit codes and refusal reasons

The result format that sweep, stash and unpack share. Back to the [README](../README.md). How to drive the tools from a script or an agent is in [driving the tools from a program](agents.md); each command's own keys and reason codes are on its tool's reference page.

## The result

**Structured output.** `--json` on every tool prints one result on stdout.

**Versioned capability contracts.** `sweep contract --json`, `stash contract
--json` and `unpack contract --json` declare observation and mutation scope,
network policy, overwrite, deletion, reversibility, persistent state and exact
supported formats. Contract queries have no application-state side effects.
Each carries `schema_version`, `tool_version`, `operation_id` and `status`.
Binary probes in the test suite compare declarations with exercised filesystem
behaviour and pin each contract's schema; deliberate schema and behaviour mutants
must fail those probes. The declaration is not a receipt or proof of no access.
**Breaking in 0.5.4 for JSON consumers:** every JSON result uses schema version 4,
including errors and refusals. Check the outer `schema_version` before parsing.
Operation fields move under `details` (for example, `details.groups`,
`details.moved`, and `details.paths_audited`). The shared envelope adds `scope`,
`observations`, `effects`, named `verification` claims, `recovery`, `disclosure`
and `reason`.
Status is `done`, `nothing_to_do`, `refused`, `incomplete` or `error`.
Read receipts separate attempted, observed, verified and failed operations,
and name unproven coverage. They contain categories and counts, never payloads,
keys or environment values. Zero counts do not prove absence of access. See [the contract specification and witness limits](contracts/README.md).

## Exit codes

**Meaningful exit codes**, uniform across the tools: `0` done · `1` nothing to
do · `2` refused · `3` error. "Refused" is distinct from "error" on purpose.
An agent must be able to tell a safety stop from a crash. Exit 0 means the whole
request was done. A missing folder gets one answer on every command: `no folder
at PATH`, exit 3, `missing_input`. `stash` refuses your home folder unless you
pass `--allow-home`.

### Whether to try again: `reason`

Every `--json` result has a top-level `reason`. It holds a stable `code` and a
`retry` class, so a caller can decide what to do next without reading the
message text:

```json
"reason": {"code": "missing_input", "retry": "after_change", "subject": {"kind": "input"}}
```

- `not_needed`: the command finished; the code is `done` or `nothing_to_do`.
- `never`: the same request cannot work, so do not send it again. The codes
  `unknown_item` and `item_assigned_twice` also carry `subject.kind: "item"`.
- `after_change`: it can work once you change something, and `subject.kind`
  says what: the input, the destination, the plan, the profile or the journal.
- `later`: the same request may work if you wait, and `subject.kind` says what
  you are waiting for.

Codes worth knowing by name: `journal_unreadable` means a saved undo record
cannot be opened (`sweep status` also gives it, with a message that says the
record is not damaged, while an apply or undo is still writing that record; run
the command again), and `journal_write_failed` means one cannot be saved.
`journal_times_tied` (`sweep undo`, `stash pop`) means two or more records have
the same time, so the tool cannot tell which one is newer and guesses nothing;
`details.same_time_records` lists each tied id with the `--id` step that opens
only that record. See [records with the same time](undo-and-journals.md#records-with-the-same-time).

`stash status --id ID --json` gives `"resume_or_undo": null` when `stash pop`
cannot restore the stash from where its record says. Before 0.5.5 the key
always held a `stash pop --id ID` step.

`sweep status` reports a record's state as `refused`, `completed`, `interrupted`,
`reversed`, `partly_reversed` or, while an apply is still running on it,
`in_progress` (with `damaged` false and `command` null); sweep declaration 18
lists these states.
`plan_stale` (exit 2) means the folder changed under the plan. A `sweep apply
FOLDER --yes` or `stash FOLDER` that loses a race with another run of the same
folder, before it moved anything, also gets `plan_stale`; scan and run again.
`profile_missing` means no saved profile exists, and `profile_invalid` means one
exists and cannot be used. In unpack, `archive_unreadable` is a damaged or
mislabelled archive, `size_limit_exceeded` is a limit reached while extracting,
`extraction_stopped` is a stop signal, `private_copy_unopened` means the system
`unzip` could not open unpack's private copy of the archive (the archive was
not read, so run the same command again), and `insufficient_space` is only ever about
the disk.

Exit codes keep their meaning. `reason` adds the part an exit code cannot
carry.

The full list is in each tool's contract: `sweep contract --json`, `stash
contract --json` and `unpack contract --json` list every code under
`details.refusal_codes`, with its retry class, its subject kind, the operations
it applies to and one line on why. `scripts/check-tool-contracts.py` makes each
tool return the codes it lists and compares the answer with the contract. It
prints which codes it proved and names the ones it could not reach. A contract
lists only codes the tool has a code path for; the few the check cannot reach
need a missing login keychain, an internal fault or a race it cannot stage.

## Progress

Every operation that takes time or writes takes `--progress-json`: `sweep apply`, `review` and `undo`, `stash`, `stash select`, `stash pop` and `restore`, and `unpack` extraction and `--list`. One shared layer writes versioned, bounded records on stderr; `--json` stdout remains one result envelope. Records distinguish planned entries, completed steps and journal records written, say so when a count is not known yet, and never state a count nobody observed. Unpack still parses nothing while extracting: it counts planned members in its private staging, from the walk its soft limit already makes. See [structured progress](progress.md) for the schema, what a killed run leaves, and measured cost. Journal records are ordered behind their moves with a barrier sync and a finished run is flushed once; see [undo and journals](undo-and-journals.md).
