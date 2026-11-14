# Driving the tools from a program

How a script or an agent runs sweep, stash and unpack, and which safety rules hold no matter who drives them. Back to the [README](../README.md). The exact result format is in [JSON and exit codes](json-and-exit-codes.md).

## For agents as well as people

These tools are built to be driven by both. The agent-facing surface is deliberate,
not incidental.

`--json` on every tool emits the same data the human rendering is drawn from.
A tool that tells a person one thing and an agent another is the worst kind of
interface.

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

An agent reads the exit code first, then the `reason` in the result: its
`retry` class says whether to send the same request again. See
[whether to try again](json-and-exit-codes.md#whether-to-try-again-reason).

## The refusals are the guardrail

The refusals are the guardrail, not the operator's judgment. Every safety
property holds no matter who is driving:

| Gate | Effect on an agent |
|---|---|
| `--inspect-content` needs a TTY | convenience gate against accidental non-interactive use, not a security boundary against a process driving a pty |
| `review` needs a TTY | convenience gate against accidental non-interactive use, not a security boundary against a process driving a pty |
| sweep sensitive-name refusal | sweep leaves a tax document alone even with `--yes`; stash still moves everything |
| per-tool journals | an agent cannot undo the other tool's work by accident |

## The agent supplies judgment; sweep supplies custody

`unknown_extensions` tells an agent what sweep has no rule for. The agent decides whether four
`.bpy` files deserve a folder -- that is the nondeterministic part, spent
where judgment is the job -- and comes back with `--map bpy=Blender`. Sweep
executes the mapping as one more table row, this run only: every refusal
still wins over a map, the moves are journaled, `undo` reverses them, and
`--map` with `--no-journal` is refused outright. If the folder does not
already exist, the output says so plainly: *the folder name Blender was
chosen by your agent, not derived from your files* -- the one sanctioned
exception to the naming rule, and it announces itself.

## `--json` discloses less, not more

For files that look like personal
records, the JSON carries counts by category and **never the paths**. An agent
gets "3 tax documents were left alone", not a list of which files those are.
Handing over that index is exactly what the naming rule exists to prevent.

## Commands that wait

**Waits with no time limit (0.5.5).** `sweep undo`, `stash pop` and
`stash status --id ID --json` wait while another run holds the same undo
record, and the wait has no time limit. A script that must not block should run these
commands with its own timeout.

## Progress while a command runs

Large apply, undo, stash, pop and unpack extraction operations report item-count milestones to
stderr, leaving JSON on stdout untouched. Small operations stay quiet; large
ones produce at most ten updates. A program that wants counts it can parse
passes `--progress-json`; see [structured progress](progress.md).
