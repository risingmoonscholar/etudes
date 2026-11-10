# JSON, exit codes and refusal reasons

How a program reads the results of sweep, stash and unpack. Back to the [README](../README.md).

## For agents as well as people

These are built to be driven by both. The agent-facing surface is deliberate,
not incidental.

**Structured output.** `--json` on every tool, emitting the same data the human
rendering is drawn from. A tool that tells a person one thing and an agent
another is the worst kind of interface.

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

```sh
sweep ~/Desktop --json          # the plan, including projects_skipped,
                                # downloads_skipped, packages_skipped, and
                                # unknown_extensions -- counts per extension
                                # sweep has no rule for. blocked_folders appears
                                # when a file holds a group folder's name: the
                                # scan still exits 0, and apply will refuse
stash ~/Desktop --for 3d --json # what moved, and when it is due
stash status --json             # what is held, and whether it is overdue
unpack a.zip --list --json      # inspect an archive without extracting
unpack a.zip --json             # what happened, or why it was refused
```

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
cannot be opened, and `journal_write_failed` means one cannot be saved.
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

**The refusals are the guardrail, not the operator's judgment.** Every safety
property holds no matter who is driving:

| Gate | Effect on an agent |
|---|---|
| `--inspect-content` needs a TTY | convenience gate against accidental non-interactive use, not a security boundary against a process driving a pty |
| `review` needs a TTY | convenience gate against accidental non-interactive use, not a security boundary against a process driving a pty |
| sweep sensitive-name refusal | sweep leaves a tax document alone even with `--yes`; stash still moves everything |
| per-tool journals | an agent cannot undo the other tool's work by accident |

**The agent supplies judgment; sweep supplies custody.** `unknown_extensions`
tells an agent what sweep has no rule for. The agent decides whether four
`.bpy` files deserve a folder -- that is the nondeterministic part, spent
where judgment is the job -- and comes back with `--map bpy=Blender`. Sweep
executes the mapping as one more table row, this run only: every refusal
still wins over a map, the moves are journaled, `undo` reverses them, and
`--map` with `--no-journal` is refused outright. If the folder does not
already exist, the output says so plainly: *the folder name Blender was
chosen by your agent, not derived from your files* -- the one sanctioned
exception to the naming rule, and it announces itself.

Large apply, undo, stash, pop and unpack extraction operations report item-count milestones to
stderr, leaving JSON on stdout untouched. Small operations stay quiet; large
ones produce at most ten updates.

**`--json` discloses less, not more.** For files that look like personal
records, the JSON carries counts by category and **never the paths**. An agent
gets "3 tax documents were left alone", not a list of which files those are.
Handing over that index is exactly what the naming rule exists to prevent.

## Progress

Every operation that takes time or writes takes `--progress-json`: `sweep apply`, `review` and `undo`, `stash`, `stash select`, `stash pop` and `restore`, and `unpack` extraction and `--list`. One shared layer writes versioned, bounded records on stderr; `--json` stdout remains one result envelope. Records distinguish planned entries, completed steps and journal records written, say so when a count is not known yet, and never state a count nobody observed. Unpack still parses nothing while extracting: it counts planned members in its private staging, from the walk its soft limit already makes. See [structured progress](progress.md) for the schema, what a killed run leaves, and measured cost. Journal records are ordered behind their moves with a barrier sync and a finished run is flushed once; see [undo and journals](undo-and-journals.md).
