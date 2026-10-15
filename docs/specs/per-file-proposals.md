# Per-file proposals for sweep

Status: design for version 1 of `sweep propose`. The decisions at the end of this document are settled.

## The problem

A caller that wants to organise a folder with its own judgment can say only two things today: route an extension to a folder (`--map`, or a saved existing-folder profile), or accept sweep's built-in groups. It cannot say "this PDF belongs in `Reading`, that one in `Taxes-2025`". An agent that reads file names, or a person with a list, has no way to hand sweep a decision about one file.

## The proposal in one paragraph

Sweep lists the files it is willing to move, each with an opaque item id bound to one exported plan. The caller sends back a proposal: for some items, a destination folder; for others, an explicit skip. Sweep checks the proposal against the plan, refuses anything it cannot justify, and writes a new bound plan with its own digest. That plan is applied by the existing `sweep apply --plan` path. No new way to move files is added.

## Who decides what is eligible

**The tool decides, from its own scan. The caller never supplies eligibility.**

The eligible set of a plan is every file the plan already proposes to move, plus every file it left alone only because no group claimed it (`no_clear_group_paths` in the plan's JSON). It never contains a file that looks personal, a file changed too recently to judge, a download still in flight, a file inside a project folder or an opaque package, a symlink, or a hidden entry. Those are counted in the plan and never listed by name, so a proposal cannot name them: there is no id to name.

The eligible set is frozen when the plan is exported and is bound to the plan digest. A proposal that cites a different digest is refused as stale.

This closes a gap found in review: binding a plan checks identity and uniqueness, not permission. A caller that edits a plan's groups before binding can currently get a personal-looking file moved. A proposal cannot do that, because eligibility is derived below the proposal layer, not read from it.

## The interface

    sweep PATH --since 0 --export-plan PLAN --json
        # existing. The result lists the eligible items:
        #   details.items = [{ "id": "i-000017", "name": "report.pdf", "size": 1832, "group": "Documents" | null }, ...]

    sweep propose --plan PLAN --plan-digest D --proposal FILE --export-plan OUT --json
        # new. A preview: moves nothing. Writes a new bound plan to OUT.

    sweep apply --plan OUT --plan-digest D2 --yes --json
        # existing, unchanged.

An item id is an index into the plan's frozen eligible list. It means nothing outside that plan, and it is not a path.

The proposal file is JSON, bounded to 1 MiB and 5,000 decisions:

    {
      "schema_version": 1,
      "plan_digest": "<digest of the exported plan>",
      "rest": "leave",                     // required; the only value in version 1
      "decisions": [
        { "item": "i-000017", "folder": "Reading",  "mode": "require_existing" },
        { "item": "i-000019", "skip": true }
      ]
    }

## Rules, each with its refusal code

| Rule | Refusal code | Retry |
|---|---|---|
| The proposal cites the digest of the plan it was built from | `plan_stale` | `after_change` |
| Every item id exists in the plan's eligible list | `unknown_item` (new) | `never` |
| An item is named at most once, even if both decisions agree | `item_assigned_twice` (new) | `never` |
| A folder is one plain name: no separator, no `..`, not absolute, not empty | `destination_outside_root` | `never` |
| `require_existing` needs the folder to exist now, and binds its identity | `profile_missing` | `after_change` |
| Version 1 has only `require_existing`; a name alone never creates a folder. A non-folder holding the name refuses | `destination_exists` | `after_change` |
| A file keeps its name. A collision at the destination refuses | `destination_exists` | `after_change` |
| `rest` must be present and equal `"leave"`. Missing or any other value refuses | `invalid_arguments` | `never` |
| Size and count limits hold; the file is valid JSON of the stated version | `invalid_arguments` | `never` |

Two codes are new and would be added to each tool's contract, with probes, like every other code. Refusals are all-or-nothing: one bad decision refuses the whole proposal and writes no plan.

## What this adds up to, and what it does not do

- It adds one command that reads a proposal and writes a plan. The executor, the journal, undo and the status lookup are unchanged. An applied proposal is undone like any other apply.
- It reads no file contents. Content-based decisions stay behind `--inspect-content`, which a person must approve.
- It makes no model call, keeps no queue, retries nothing, and takes no request keys. The caller owns all of that. Sweep refuses or completes.
- Several workers can read the same exported plan and produce separate proposals. Combining them is the caller's job: sweep takes one proposal per run, and two proposals that name the same item are not merged by sweep.
- It does not rename files, create folders, create nested folders, or move anything outside the plan's root.

## Tests the build must come with

Each rule above gets a test that fails when the rule is broken. In addition:

1. A proposal that names a personal-looking file by guessing an id is refused, through the command line and through the library.
2. Editing the exported plan file before `propose` is refused by the digest check. Editing the proposal's digest is refused as stale.
3. A destination swapped for a link between `propose` and `apply` is refused at apply, as in the existing folder-identity check.
4. A planted fault: change one refusal's code in the source and the contract check must turn red; remove one rule's check and its test must fail. Both must be shown to fire.
5. A stress scenario: propose, apply, kill the apply part-way (pause first, confirm stopped, then kill), `sweep status` reports it, `sweep undo` restores every original path and byte.

## Decisions for version 1

- Item ids are an index into the plan's frozen eligible list, in the form `i-000017`, bound to the plan digest.
- The only mode is `require_existing`. There is no `create_if_absent` and no `rest: "keep_plan"`. The field `rest` is required and must equal `"leave"`.
- The item list includes the file name.
- A `skip: true` decision names an item that stays in place.

## Sources

- `docs/plan-binding.md`: how an exported plan is bound, digested and replayed.
- `crates/etude-core/src/plan.rs`: `Plan`, `Group`, `BoundPlan`, the `allowed_sources` set built at binding, and `to_json`, which lists `no_clear_group_paths` and counts the rest.
- `crates/etude-core/src/lib.rs`: `Untouched`, the reasons a file is left alone.
- `docs/contracts/README.md`: the `reason` field, its retry classes, and how codes are listed per tool.
