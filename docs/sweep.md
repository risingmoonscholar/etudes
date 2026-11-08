# sweep

Sweep sorts a messy folder into groups. It shows a plan and moves nothing until you run `sweep apply`. Back to the [README](../README.md).

## A plan, before anything moves

```console
$ sweep --scheme folders ~/Desktop

Scanned 108 items  ·  filesystem and Finder organization metadata  ·  no contents read to decide

  Screenshots      34 files   named "Screenshot ..."
  Photos, Jan 15   27 files   camera names, taken within 3 days
  Installers        3 files   .dmg and .pkg
  Archives          3 files   .tar, .zip
  Documents        17 files   .docx, .md, .pdf, .txt
  Images            3 files   .jpg
  Scripts           3 files   .sh

  14 files look like personal records and were not touched
  1 file changed too recently to judge and was left alone
  3 files matched no group and were left where they are

  skipped 1 hidden item and 3 symlinks

Nothing has been moved.  Nothing left this machine.
Note: this listing is in your terminal scrollback.
Review: sweep review ~/Desktop
Apply:  sweep apply ~/Desktop --yes
Applying reads the first and last 4 KiB of each file it moves, so undo can tell if one changed.
```

Nothing moved, because nothing has been confirmed yet. The tax documents, the
medical records and the driver's licence are the files reported as personal
records, and no flag moves them.

## What sweep refuses to touch

The current organization mode is named `--scheme folders` (and is the default
when the flag is omitted). A user exclusion has the highest precedence: review
can skip a group, and `apply --only NAME` selects only the named group. Safety
holds come next and remove files from every grouping rule. Built-in grouping
then claims eligible files; an agent `--map` can claim only what those built-in
rules leave behind. This is the policy order, not a claim that sweep detects
every filesystem state. The schemes are `folders`, `existing` and `tags`.

Sweep makes a new folder only for three or more files of one kind, so it does
not guess a folder from one or two files. When the folder is already a direct
folder of the selected directory (for example `Documents`), any number of files
of that kind moves into it, one or two included, and at `--depth 2` files
already inside it are reported as in place. A `--map` folder follows the same
rule. Fewer than three files with no folder of their own stay where they are,
and the plan says which folder would take them.

### Tag files, move nothing

`sweep --scheme tags` adds Finder tags and moves nothing. You name the tag for
each extension with `--map EXT=TAG`. The tag can be one you already use, a new
one an agent proposes, or a new one you asked for. Sweep never makes up a tag.

```console
$ sweep ~/Downloads --scheme tags --map pdf=Receipts --map png=Photos --export-plan ~/plan.bin

  Tags to add
  Receipts    12 files  .pdf
  Photos       4 files  .png   NEW TAG

  1 tag is new: Photos
  A tag is new when no file in the scanned folder carries it.
  Finder's own tag list cannot be read, so a tag you use in Finder but not on a file here also shows as new.

Nothing has been tagged or moved.  Nothing left this machine.

$ sweep apply --plan ~/plan.bin --plan-digest <digest> --yes
```

- **The plan is the gate.** Planning and exporting write no tag and touch no
  file. Tags are written only by `sweep apply --plan FILE --plan-digest SHA
  --yes`, for a plan you have seen. The live `sweep apply PATH` and `sweep review`
  refuse tags, and a plan made before a file's tags changed is refused as stale.
- **Tags are only added.** A file keeps every tag it has, in the same order, and
  the new tag goes after them. A file that already has the tag is left as it is.
  Nothing is moved, renamed, created or deleted.
- **Tagged files are tagged too.** Because a tag change is add-only and moves
  nothing, `--scheme tags` works on files that already carry tags without
  `--include-tagged`. That flag is refused with this scheme.
- **New tags are marked.** The plan, in words and in `--json`, lists each tag
  with its file count and marks each tag no file in the folder carries as new.
  An existing tag is added exactly as the folder spells it, colour included.
- **Unknown tag state is not guessed.** A file whose tag attribute cannot be read
  or parsed is listed and given nothing.
- **Undo restores recorded tag writes.** The journal records each file's tag
  attribute before the first write. Undo restores those exact bytes when a done
  record proves the write completed, the file has the same identity, and its
  tags still match. Replacement files and changed tags are left alone and named.
  A write without a done record is left alone and settled, since matching tag
  bytes cannot prove who wrote them. Older tag journals without file identity
  retain their original tag-byte checks. Apply needs a journal; `--no-journal`
  is refused.
- Sweep reads the tag list of the files it may tag, never their contents and
  never Finder comments. Personal-looking, recent, in-progress, project and
  hidden files are skipped, as in every scheme. `--scheme tags` needs macOS.

### Use folders you already have

`sweep --scheme existing PATH` reads the plain profile
`~/Library/Application Support/etudes/sweep-existing-profile.toml` (or the same
filename under `ETUDE_STATE_DIR` when that variable is set).
Use `sweep apply PATH --map bpy="Blender Files" --save-routes --yes` to remember
validated routes after every planned move succeeds. The destination must already
be a direct visible folder. Conflicts refuse before moving anything; failed or
partial applies save no routes. Saving needs a journal and cannot use tags.
Sweep adds new routes after your own text, privately and atomically, and leaves
the file alone when every route is already saved. Undo moves files back but
keeps saved routes; edit the profile to drop one. You can also add routes
as versioned data you can review before use:

```toml
schema_version = 1

[[routes]]
extension = "bpy"
folder = "Blender Files"

[[routes]]
extension = "pdf"
folder = "Reading"
```

A route's folder is a direct, visible folder in the selected directory. When
it is missing, sweep creates it, the same way as a `--map` folder, but only when
the run moves at least one file into it. The plan lists the folder before
anything moves, and undo removes it again if it is empty after the files go
back. A route whose extension has no files here is ignored and creates nothing.
If a file or a link holds the folder's name, sweep refuses before anything
moves and names it. A route moves any number of matching files, one or two
included: a saved route is your own choice, so the three-file floor for new
folders does not apply to it. No built-in rule takes a routed file. Each
extension needs its own destination. Duplicate or invalid routes refuse before anything
moves. Safety refusals still take precedence over the profile; eligible profile
routes take precedence over built-in grouping. A routed file is claimed once,
even when its name also matches a built-in detector such as Screenshots. At
`--depth 2`, files already directly inside their route destination are reported
as in place and are not proposed again; deeper files remain eligible for
promotion into that destination. Sweep records the profile's SHA-256 in an
exported plan, and refuses replay if the profile changes.

| State | Current behavior |
|---|---|
| locked | An unreadable subtree is counted and the scan reports incomplete; visible siblings remain usable. A selected folder that cannot be read at all is an error (exit 3, `permission_denied`) that names the folder and what to do next. |
| immutable | A file or folder with the locked flag (Finder's Locked box, `chflags uchg` or `schg`) is left in place and counted, and the summary says how to clear it. A folder with the flag is not entered. Apply checks again before each move. A locked root or destination folder refuses the plan before any move. Sweep reads the flag from the metadata it already has. |
| invisible | Hidden entries are skipped and counted. Unreadable entries are separately reported as incomplete. |
| alias | Finder aliases are skipped by the kIsAlias flag in FinderInfo, without resolving them or reading their contents. Symlinks are skipped during scans; selected parent aliases are resolved for overlap checks. |
| iCloud placeholder | Dot-prefixed `.icloud` stubs are skipped as hidden entries, not grouped as their missing original file. |
| coordinated write | Recognized in-flight filename suffixes and recent modification times are held. Sweep does not query File Provider coordination, so those checks do not prove that no coordinated write is active. |

The implementation declares the same precedence and state accounting in
`etude_core::plan`; regression tests exercise safety holds against `--map` and
keep the named state inventory reviewable.

The four main plan-level refusal groups are named in the output with counts.
A count with no reason beside it is the defect this project keeps finding in
itself.

**Finder-tagged items.** A file or folder with a Finder tag is organisation
you may be using, so sweep leaves it where it is and says how many
(`1 Finder-tagged item was left alone`). It asks only whether a tag is
present: it never reads the tag names (only `--scheme tags` does, to add to
them), never reads Finder comments, and gives a caller a count, not names or
paths. Held tagged items are not counted in the "Scanned N items" total; the
JSON plan reports them separately as `tagged_skipped`. `--include-tagged`
organises tagged items anyway; it prints a warning on stderr every time, and
the tag stays on the item through a move on the same volume. A tagged item that
would have to cross volumes stays where it is, because a cross-volume copy does
not carry the tag.
`sweep apply` checks each item's tag again just before it moves the item and
again just after, so a tag added at the last moment holds the item or moves it
back. `stash` exists to clear a folder, so it takes tagged items too, and says
the folder is clear only when it is.

**Your projects.** A folder holding `project.godot`, `Cargo.toml`, a `.flp`,
a `.blend`, a `.ptx` -- 25 markers in all -- is stepped over rather than
sorted. A Godot `.tscn` references its siblings as `res://scripts/main.gd`,
absolute from the project root, so moving any file inside one breaks every
reference to it. The list was measured against real projects on a real disk,
not assembled from vendor documentation.

**Anything macOS treats as a single item.** A `.fcpbundle`, a `.band`, a
`.logicx`, a `.app`, a Pages document. The folder holding one is ordinary and
still gets swept -- the bundle is stepped over as a unit, not made contagious.

**Downloads still arriving.** `.part`, `.crdownload`, `.download` and friends,
whether they are files or directories. Safari writes `movie.mp4.download/`
with the partial data inside it.

**Anything touched in the last day.** A file you are working on right now is
not a file to be filed. `--since 6h` narrows the window, `--since 0` turns it
off, and an unreadable value is an error rather than a silent fallback to the
default.

```console
$ sweep ~/Downloads

  Documents       12 files   .pdf
  1 folder was left alone because it holds a project file
  2 files changed too recently to judge and were left alone
  1 download is still in progress and was left alone
```

### What it does not protect

Sweep decides from filesystem and Finder organization metadata only; it never
reads file contents to decide. When it moves a file it reads the first and last
4 KiB, so undo can tell if that file changed. If a Finder tag lands on a file
while sweep is moving it and the file cannot be moved back because its original
place has been taken, sweep leaves it where it is, counts it as an item it
could not return, and records that in the journal. With `--scheme tags` it reads
each taggable file's tag list and writes it back with one more tag; it moves
nothing.

That has a cost worth stating plainly. Sweep reads whole contents only if you
pass `--inspect-content` and consent, or when a move crosses volumes and the
file has to be copied.

A project *document* -- a `.blend`, an `.flp`, an `.als` -- references its
assets relative to itself and freely upward, out of its own folder. Since
0.5.4, a document found during the scan also holds related asset families
beside it: a `.blend` in `scenes/` keeps sibling textures in place. Unrelated
documents and screenshots can still be organized. This protection follows
filesystem names and layout; it does not parse the project's references or
promise to discover documents outside the scan's scope.

Similarly, a Final Cut library told to keep its media *outside* the bundle has
no filesystem-level mark saying which library owns that media. That
relationship lives in the library's own database. Sweep protects managed
layouts; it cannot protect an arrangement only the application knows about.

## Rules the tools share

**Never coin a label the filesystem did not already contain.** A folder named
`Tax return 2024` is itself a disclosure, visible in Finder, indexed by
Spotlight, captured by every backup. Group names come only from words your own
filenames already carry. If *you* want a revealing name, `sweep review` will let
you choose one after telling you what it costs.

**Reading more must mean acting less.** `sweep --inspect-content` is off by
default and needs consent separate from `--yes`. What it reads can only ever
move a file into "left alone". It never influences a destination.

**Apply the plan you exported.** `sweep PATH --export-plan FILE --json` and
`stash PATH --export-plan FILE --json` save explicit private metadata artifacts
without moving anything. Replay with `--plan FILE --plan-digest SHA` binds the
original root, file identities, configuration, tool version and observation
contract. If those observations changed, it refuses before probes, journals or
moves and tells you to make a new plan. Direct apply still creates a fresh proposal in that invocation.
See [plan binding and its metadata limits](plan-binding.md).

**Place individual files.** `sweep propose` reads your folder choice for each
file you name in an exported plan and writes a new plan. It moves nothing. The
export lists the files sweep is willing to move in `details.items`, each with an
`id`, `name`, `size` and the `group` it would join (`null` when no group claims
it). A file that looks personal, changed too recently, is still downloading,
sits in a project folder or package, is a link or is hidden is never listed, so
a proposal cannot name it.

```sh
sweep ~/Desktop --since 0 --export-plan ~/desktop.plan --json
# proposal.json
# {"schema_version":1,"plan_digest":"SHA","rest":"leave","decisions":[
#   {"item":"i-000004","folder":"Reading","mode":"require_existing"},
#   {"item":"i-000005","skip":true}]}
sweep propose --plan ~/desktop.plan --plan-digest SHA --proposal proposal.json \
  --export-plan ~/desktop-proposed.plan --json
sweep apply --plan ~/desktop-proposed.plan --plan-digest SHA2 --yes --json
```

The folder must already exist directly under the plan's root and is one plain
name. A file keeps its name. Files the proposal does not name stay where they
are, and `rest` must say `"leave"`. One bad decision refuses the whole proposal
and writes no plan. A proposal is at most 1 MiB and 5,000 decisions. The plan it
writes applies, and undoes, like any other. `sweep propose` reads no file contents.
