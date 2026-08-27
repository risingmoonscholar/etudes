# Finder and `scrub`: adversarial assessment and frozen protocol

Status: **protocol only; no product implementation is authorized by this document**  
Frozen: 2026-08-24 on macOS 26.2 / Xcode 26.4.1  
Repository state examined: `nightwatch-voice-gate`, clean worktree

This document fixes the claims, tests, and kill criteria that must be decided
before Finder integration or `scrub` is implemented. Changing a frozen claim or
kill criterion requires a new protocol revision, committed before the product
change that relies on it. A failed or uninspectable claim is not silently
reworded into a pass.

## 1. Decision summary

Use Finder Quick Actions as the entry point, implemented as signed macOS Action
Extensions inside a minimal containing `Etudes.app`. Do not use Finder Sync.
Do not ship Automator, Shortcuts, Services, AppleScript, or shell workflows as
the trusted adapter.

Quick Actions can plausibly be the entire *entry-point* interface. They are not
yet proven to be the entire execution environment. Two platform boundaries must
pass experiments first:

1. An Action Extension receives inputs through `NSItemProvider`; a local item
   may be opened in place, copied to a temporary representation, or materialized
   by a File Provider. A path is not proof that a sibling is writable.
2. A sandboxed child launched with `Process` inherits static sandbox rights, but
   not dynamic access granted after launch, such as Powerbox access. Therefore
   “pass the selected path to the bundled Rust binary” is **unproven** until the
   child can read the input and safely create the intended sibling output on all
   supported locations. Apple documents bookmarks or passing data as the ways
   to bridge that gap.

The first vertical slice is **Open Archive Safely**, because it exercises an
existing engine, structured outcomes, refusal rendering, selected-file access,
and a created output without adding metadata semantics. It is still a platform
spike before it is a feature. If returning an extracted directory through the
Action Extension contract cannot reliably place it beside the archive, use a
short-lived operation UI owned by the containing app or kill this integration;
do not add a daemon or broaden filesystem entitlements.

The recommendation for `scrub` is **narrow**:

- Research and, only after the gates pass, build a JPEG-only v0.1.
- HEIC is outside v0.1 unless its separate preservation gates all pass. A
  JPEG-only release is preferable to a HEIC claim that changes HDR or auxiliary
  content.
- Use the Finder label **Remove Hidden Photo Metadata**. “Make a Private Copy”
  implies protection against visible private content that the tool does not
  provide. The shorter marketing sentence may appear only beside the mandatory
  warning, never as the contract.

## 2. Repository evidence that constrains the design

The current repository has 45 graph-indexed source files, 670 graph nodes, and
21 communities. The graph identifies scan/plan/apply/journal and CLI dispatch as
the central seams. `cmd_apply` rebuilds a plan immediately before mutation;
`cmd_stash` builds one accepted collection and uses the same apply engine;
`unpack::run` performs list, judgement, destination preflight, bounded extract,
and cleanup in one path. The Finder layer must call these paths, not reproduce
their policy in Swift.

Existing invariants to preserve:

- `0` completed, `1` nothing to do, `2` refused, `3` error.
- Human and JSON output come from the same result data.
- `sweep apply` rescans instead of trusting a stale preview.
- `sweep review` and content inspection require explicit interaction; a Finder
  button is not implicit consent.
- `stash` records a due time but has no background restoration.
- `unpack` handles one archive per invocation, refuses unsafe members before
  extraction, never silently replaces its destination, bounds writes by actual
  disk consumption, and removes a partial destination on failure.
- The stress harness has three outcomes—passed, failed, unproven—and treats a
  scenario with no assertions as failed. New witnesses inherit this rule.
- Runtime networking is denied by an OS sandbox witness; no Finder target or
  `scrub` target may add a network entitlement or network-capable dependency.

The four open issues were read and change the plan:

- [#73](https://github.com/risingmoonscholar/etudes/issues/73): a project
  document in one subfolder does not protect sibling assets. Finder must show
  project/package/recent/unknown refusals and may not turn “Organize” into
  unconditional `--yes`.
- [#44](https://github.com/risingmoonscholar/etudes/issues/44): Finder tags are
  deliberately outside sweep's “names, sizes and dates only” promise. The
  Finder adapter must not inspect tags and quietly change sweep policy.
- [#33](https://github.com/risingmoonscholar/etudes/issues/33): a safety cleanup
  can exist without a witness that its destructor invokes it. Any new sensitive
  buffer or temporary artifact needs a call-path witness, not only a helper
  unit test.
- [#12](https://github.com/risingmoonscholar/etudes/issues/12): journaled apply
  can take about a minute for 10,000 files. Long-running Finder behavior and
  progress are product requirements, not polish.

The graph's documented wiki path, `graphify-out/wiki/index.md`, is absent in
this checkout. The current `GRAPH_REPORT.md`, `graphify-brief` results, README,
SECURITY, manifests, CLI help, all test names and relevant process-level tests,
the stress runner/library/baseline/scenarios, CI, claim checks, and surface
inventory were inspected.

## 3. macOS integration mechanisms

| Mechanism | What Apple supports | Fit | Decision |
|---|---|---|---|
| Action Extension shown as a Finder Quick Action | Finder supplies typed attachments through `NSExtensionContext`; an extension may have focused native UI, complete or cancel a request, and return file representations. Apple's sample contains a plumbing app plus multiple extensions. | Direct selection, native UI, typed inputs, no Terminal. It has lifecycle, sandbox, and long-operation limits that must be measured. | **Primary candidate** |
| Automator Quick Action workflow bundle | A workflow receiving files appears in Finder, Services, and the Preview pane and can be enabled in Extensions settings. | Easy prototype, but shell/AppleScript actions invite string interpolation, workflows are user-editable, result/error UI is weak, and signed upgrades are not a clean product boundary. | Test oracle or developer prototype only; do not ship |
| Services / `NSServices` | An app can advertise services, pasteboard types, and a response timeout; Quick Actions may also appear in Services. | Broader and older menu surface, weaker selection/result contract, no advantage over the Action Extension. | Compatibility comparison only |
| Shortcuts | A user-created shortcut can receive files/folders and be enabled as a Finder Quick Action. | Adds a mutable orchestration layer and permissions outside the versioned product. Useful for users composing their own automation, not for the trusted default. | Optional later adapter, never the safety boundary |
| App Intents | App actions become available to Shortcuts and other system experiences. Apple currently states App Shortcuts themselves are not supported on macOS; users can build custom shortcuts from macOS intents. | No direct, preconfigured Finder-selection surface on macOS. It does not replace an Action Extension. | Defer |
| Finder Sync | Badges, contextual menus, and toolbar items for monitored directories, primarily for synchronization software. Apple says it is not a general Finder UI mechanism and notes long-lived instances. | Requires monitored roots and encourages a long-running extension/service architecture contrary to the product. | **Reject** |
| File Provider extension/action | Actions for items managed by that provider; File Provider is for making remotely managed documents available. | Does not apply to arbitrary local selections and would turn Études into a storage provider. | **Reject** |
| Minimal containing app | Required/recommended plumbing for extension distribution and Gatekeeper approval; nested code can be signed and notarized together. | Acceptable if it only reports version, extension status, privacy posture, and removal instructions. No file-management window. | **Required plumbing** |

Official references:

- [Add Functionality to Finder with Action Extensions](https://developer.apple.com/documentation/appkit/add-functionality-to-finder-with-action-extensions)
- [Use Quick Action workflows on Mac](https://support.apple.com/guide/automator/use-quick-action-workflows-aut73234890a/mac)
- [Run a shortcut while working on your Mac](https://support.apple.com/guide/shortcuts-mac/apd163eb9f95/mac)
- [App Shortcuts, macOS platform note](https://developer.apple.com/design/human-interface-guidelines/app-shortcuts)
- [Finder Sync extension guide](https://developer.apple.com/library/archive/documentation/General/Conceptual/ExtensibilityPG/Finder.html)
- [Finder extension settings](https://support.apple.com/guide/mac-help/mtusr003/mac)
- [App Sandbox file access](https://developer.apple.com/documentation/security/accessing-files-from-the-macos-app-sandbox)
- [Embedding a command-line tool in a sandboxed app](https://developer.apple.com/documentation/xcode/embedding-a-helper-tool-in-a-sandboxed-app)
- [Notarizing macOS software](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)

## 4. Proposed package and lifecycle

### Bundle

Ship one universal, hardened-runtime, Developer ID-signed and notarized
`Etudes.app` with stable bundle identifiers:

```text
Etudes.app/
  Contents/MacOS/Etudes                 minimal plumbing executable
  Contents/PlugIns/
    Organize.appex
    PutAside.appex
    BringBack.appex
    OpenArchiveSafely.appex
    RemovePhotoMetadata.appex
  Contents/Helpers/
    sweep
    stash
    unpack
    scrub                              only after scrub gates pass
```

The extension display names are the Finder labels. A shared Swift adapter may
normalize `NSExtensionItem` inputs and render result models, but it must not
classify, approve, choose destinations, parse archives, or decide metadata
policy. Every helper is built from the same commit as the app, signed as nested
code, launched by absolute bundle URL with a `[String]` argv array through
`Process`; never `/bin/sh -c`, AppleScript, `osascript`, PATH lookup, or a
constructed command string. Standard output and error are bounded and captured
separately.

Because post-launch sandbox grants do not automatically reach a child, the
experiment must choose and witness one of these bridges:

1. a bookmark or file descriptor passed explicitly to a narrowly extended CLI;
2. a Rust library linked in-process behind the exact same CLI outcome model; or
3. an Apple-documented output representation that Finder places, with the CLI
   operating only in an extension-owned replacement directory.

Option 2 is allowed only if CLI and Finder call the same exported engine and
the equivalence witness proves identical outcomes. A second Swift policy
implementation is forbidden.

### Install and enable

Prefer a notarized DMG containing `Etudes.app` and a plain instruction to drag
it to `/Applications`; it avoids privileged installer scripts. On first open,
the plumbing app performs no file operation. It shows:

- version and build identity;
- “Nothing leaves this Mac” and the visible-content warning;
- whether each Action Extension is enabled;
- the exact System Settings path for enabling/disabling it;
- removal and journal-retention behavior.

Direct distribution requires the user to open and approve the containing app
before Gatekeeper allows its extension. The Finder actions are disabled by
default until the user enables them in Login Items & Extensions > Finder. Do
not claim zero-click installation.

### Upgrade and removal

Upgrade by replacing the app bundle with a newer notarized bundle while
retaining bundle identifiers. Test upgrade from the oldest supported version,
enabled and disabled states, Finder relaunch, logged-out/logged-in state, and
in-flight operation refusal. Never update executable code in place or download
an updater.

Removal is dragging `Etudes.app` to Trash, followed by verification that all
actions disappear after Finder/extension registry refresh. Journals are user
recovery material and are **retained** by default; uninstall must say where
they remain. Deleting journals is a separate explicit action using the existing
key-sharing warning. No uninstaller may recursively delete an inferred path.

## 5. Finder action contracts

### Shared adapter contract

- Input is an ordered list of typed file URLs/representations, never shell
  text. Preserve arbitrary Unicode and control bytes representable by macOS.
- Resolve/materialize each item, coordinate access where required, and retain
  security-scoped access only for the operation.
- One CLI invocation per selected archive or folder unless that CLI gains a
  native batch contract. Batch isolation belongs in a shared outcome combiner,
  not shell loops.
- Map exit codes without reinterpretation: completed, nothing to do, refused,
  error. Mixed-batch precedence is `error (3) > refused (2) > completed (0) >
  nothing (1)`, while JSON retains every per-input result.
- The native result view is rendered from the parsed structured outcome. If an
  old helper emits an exit code without a complete JSON error object, wrap
  stdout/stderr as bounded opaque text and mark fields unproven; do not infer a
  friendlier reason.
- Do not request notification permission merely to show a result. Prefer the
  extension's focused result UI. If later notifications are justified, default
  text contains counts and outcome class, not filenames or metadata values.
- “Show in Finder” may use
  [`NSWorkspace.activateFileViewerSelecting`](https://developer.apple.com/documentation/appkit/nsworkspace/activatefileviewerselecting(_:))
  after success. It is not Finder automation and needs no persistent app.
- Cancellation sends an interrupt to the child, waits a bounded grace period,
  then terminates it. The engine remains responsible for journal/temporary-file
  recovery. Cancellation is not reported as success.

### Open Archive Safely

- Accept one or more supported archives; invoke `unpack ARCHIVE --json` once
  per input.
- Default destination remains beside each archive and remains engine-chosen.
- Existing destination, unsafe member, size bound, `.dmg`, and incomplete type
  listing stay refusals.
- Display a per-archive result and a batch summary. Offer Show in Finder only
  for destinations actually reported by successful engine outcomes.
- File Provider items that are not materialized, cannot yield a stable local
  parent, or cannot accept a returned directory are refused honestly.

### Put Aside / Bring Back

- v0.1 Put Aside uses **no deadline**. This is the existing strong default and
  avoids adding a prompt whose only effect is a reminder the product cannot
  act on automatically.
- A later “Put Aside Until…” may offer a small duration prompt, but must say
  “due” and “remind,” never “automatically restored.”
- Bring Back invokes `stash pop SELECTED_FOLDER`. It does not search all
  journals or reveal paths outside the selected folder.
- Hidden-item counts and early/damaged-journal disclosures remain visible.

### Organize This Folder…

This action always has a review step. It may never invoke `sweep apply --yes`
directly from the menu.

The smallest acceptable native sheet presents, from `sweep PATH --json`:

- proposed groups with counts and naming signal/provenance;
- counts for personal records, never their paths;
- recent, in-flight, project, package, system-policy, unreadable, hidden, and
  symlink counts;
- unknown extensions by extension and count;
- synced-root refusal/consent state;
- an explicit checkbox or selection for every group to be applied.

Approval then invokes an engine operation that rescans and accepts only the
chosen group identities. If the filesystem changed, the new plan is shown
again or the engine refuses. Agent-chosen `--map` names are not part of Finder
v0.1. Content inspection stays absent until a native consent flow can preserve
its separate opt-in and memory-cleanup witnesses.

## 6. Frozen `scrub` v0.1 contract

User sentence:

> Create a new JPEG copy with supported hidden photo metadata removed. The
> original is unchanged. Visible details in the image are not inspected.

Finder label: **Remove Hidden Photo Metadata**  
CLI: `scrub INPUT... [--json]`

Mandatory result warning, shown on every completed output and never hidden
behind help:

```text
Hidden metadata was removed.

Scrub does not inspect the image itself.
Faces, addresses, documents, reflections, screens,
license plates and other visible details remain.
```

### Supported input

v0.1 supports only a decodable, single-image JPEG whose container and metadata
are completely classified by the implementation and the independent witness.
Extension and Uniform Type Identifier are hints; the byte signature and decode
must agree. Symlinks, packages, directories, malformed data, polyglots, and
unknown application segments are refused or errored as classified below.

For each input, `scrub`:

1. opens the input without following a symlink and records a stable identity
   and content fingerprint;
2. creates a sibling name `<stem>-private.<original-extension>`, preserving the
   original extension spelling;
3. refuses any case-fold or normalization collision and never silently chooses
   `-private-2`;
4. writes a mode-0600 temporary file in the destination directory;
5. independently reopens and verifies the temporary output;
6. rechecks that the input identity/content did not change;
7. fsyncs the output, places it with an atomic no-replace operation where the
   filesystem supports one, and fsyncs the directory;
8. removes the temporary file after every pre-placement failure.

Cross-volume placement is not needed for a sibling and is refused if observed.
The original mode, extended attributes, ACLs, quarantine, Finder tags, and
creation date are not copied. Output file modification time is the creation
time of the private copy. This is stated behavior, not accidental loss.

### Exact metadata policy

Remove:

- the entire GPS dictionary and corresponding XMP GPS properties;
- XMP packets and extended XMP;
- IPTC/IIM and Photoshop resource metadata;
- EXIF maker notes, user comments, owner names, unique IDs, camera/body/lens
  make, model, serial, firmware, and editing/software history;
- EXIF and JFIF embedded thumbnails;
- JPEG comments and unclassified descriptive/application metadata;
- capture date/time and timezone. Maintainer ruling 2026-08-25: the original
  brief's example kept the capture date; it is removed instead. A date can
  identify an event — a party, a clinic, a protest — and is not
  rendering-critical. Anyone who wants the recipient to know the date can say
  it out loud.

Preserve only what is required to render the same photograph:

- encoded image dimensions and image samples under the equivalence definition
  below;
- intended EXIF orientation, including mirrored values 2, 4, 5, and 7, or a
  demonstrably equivalent normalized representation;
- ICC profile and structural color information needed for equivalent rendering;
- structural JPEG/JFIF/Adobe markers proven necessary for decoding/color.

Do not report “all metadata removed.” Report the exact removed classes and the
exact rendering properties preserved. ICC profiles may themselves contain
descriptive strings; the UI must say “color profile retained for rendering.”

### Pixel and color equivalence

The frozen requirement is that JPEG entropy-coded image data stays
byte-for-byte unchanged while only metadata/container segments are rewritten.
Two implementations may satisfy it, in order of preference:

1. Apple Image I/O, if experiment shows it can rewrite metadata without
   re-encoding. This is not assumed: `CGImageDestination` is a re-encoder by
   design, and no Apple documentation promises lossless JPEG rewriting.
2. A closed, minimal segment-level container rewrite: walk the JPEG marker
   structure, copy entropy-coded data verbatim, drop the disallowed segments,
   refuse on anything outside the allowlist. This is the probable design and
   the S2 witness applies to it identically.

Path 2 must be named for what it is: **the first Études code that parses a
binary format inside a mutation path.** Every existing tool dispatches to
platform software and refuses to parse the bytes it moves; that stance is
load-bearing in SECURITY.md. The break is accepted here because the JPEG
marker walk is a short length-prefixed scan, the allowlist is closed, and the
unknown case refuses — but it is a break, it must be stated in SECURITY.md if
scrub ships, and it does not license parsing anywhere else in the suite.

A lossy re-encode satisfies nothing. It is acceptable **only after a protocol
revision** that replaces “same image samples” with a measured visual-loss
claim and makes the lossy operation unavoidable in the UI. v0.1 as frozen here
is killed rather than quietly re-encoding.

Orientation equivalence means decoding original and output with orientation
applied yields the same width, height, and pixel raster for all EXIF values
1–8. Color equivalence means both decode to the same tagged color space/profile
and the rendered comparison passes the frozen exact or tolerance rule. No
tolerance rule exists in this revision because no lossy path is authorized.

### Output and exit contract

Per-input structured fields are: `input` (redacted under the same path policy as
other tools), `outcome`, `output`, `removed` (class names only), `preserved`,
`warning`, and `diagnostics` without metadata values. Human output is a renderer
of this object.

- `0`: every input produced a verified private copy.
- `1`: every input was an exact no-op. Reserved and normally unreachable in
  v0.1: scrub always creates a sibling or refuses, and detecting “already
  scrubbed” would need a content-fingerprint registry the tool deliberately
  does not keep. Stated so the suite-wide meaning of `1` is not quietly
  different here.
- `2`: at least one input was refused and none errored; successful siblings may
  coexist and are individually reported.
- `3`: at least one input errored; successful siblings may coexist and are
  individually reported.

Unsupported/ambiguous format, symlink, existing sibling, unknown JPEG segment,
multi-image input, input changed during processing, or unavailable atomic
placement is a refusal (`2`). Missing input, permission failure, malformed JPEG,
decode/encode failure, disk full, or failed cleanup is an error (`3`). A cleanup
failure must name that a partial temporary item may remain without printing
sensitive metadata.

No journal is created for `scrub`: it does not alter or remove the original,
and undo is deleting the newly named sibling. A pathname journal would retain
privacy-sensitive history without enabling restoration. This follows the same
creation-only distinction as `unpack` and must be stated in SECURITY.md if the
tool ships.

## 7. Format matrix

| Format / shape | v0.1 | Remove | Preserve / reason | Refuse when |
|---|---|---|---|---|
| JPEG, single image, fully classified | Candidate support | GPS, XMP, IPTC/Photoshop metadata, identifying EXIF, comments, thumbnail, dates | entropy-coded image data, dimensions, orientation semantics, ICC/required color markers | unknown APP/COM structure, polyglot, inconsistent decode, unsupported color/orientation rewrite |
| JPEG without GPS | Support if otherwise classified | same classes; never imply GPS was present | same as above | same as above |
| HEIC/HEIF | **Refuse in v0.1** | none | none | always; separate gate required |
| HEIC with HDR gain map, depth, matte, auxiliary or multiple images | Refuse | none | none | always; semantics exceed contract |
| PNG | Refuse | none | none | textual chunks, color chunks, animation and ancillary-chunk policy not specified |
| TIFF | Refuse | none | none | multi-page/thumbnail/EXIF/IFD graph not specified |
| WebP | Refuse | none | none | animation/EXIF/XMP/ICC policy not specified |
| GIF/APNG/animated image | Refuse | none | none | animation/timing/frame semantics |
| RAW/DNG | Refuse | none | none | original sensor and preview/maker-note semantics |
| Live Photo | Refuse | none | none | paired image/video asset and relationship metadata |
| PDF | Refuse | none | none | document metadata is not photo metadata; signatures, forms, attachments |
| Any format with a misleading extension | Refuse/error from signature/decode result | none | none | declared and observed types disagree |

HEIC may enter a later revision only for a shape explicitly named by the
contract. Apple exposes image count, primary image, auxiliary depth/disparity/
matte data, and HDR gain maps, and provides
`kCGImageDestinationPreserveGainMap`; those APIs show that “HEIC” is not one
simple image. Presence, absence, and byte/render equivalence of every supported
auxiliary item must be independently witnessed.

Image I/O references:

- [`CGImageDestination`](https://developer.apple.com/documentation/imageio/cgimagedestination)
  documents inherited source properties, metadata merge/exclusion, orientation,
  thumbnail, GPS/XMP exclusion, and gain-map preservation controls.
- [`CGImageSource`](https://developer.apple.com/documentation/imageio/cgimagesource)
  exposes type, image count, primary image, properties, and auxiliary data.
- [EXIF dictionary keys](https://developer.apple.com/documentation/imageio/exif-dictionary-keys)
  include owner, body/lens serial, maker note, software-adjacent, and unique-ID
  fields.
- [IPTC dictionary keys](https://developer.apple.com/documentation/imageio/iptc-dictionary-keys)
  include creator/contact/location/source and software fields.
- [HDR gain map data](https://developer.apple.com/documentation/imageio/kcgimageauxiliarydatatypehdrgainmap)
  is an auxiliary image, not ordinary descriptive metadata.

## 8. Privacy and filesystem threat model

### Assets

- original bytes and render semantics;
- hidden identifying metadata and values;
- visible content, which remains out of scope but must never be implied safe;
- filenames and selected paths;
- journal/keychain recovery state;
- output integrity and honest per-item results.

### Adversaries and failures in scope

- a crafted archive or image exploiting parser ambiguity, size, count, nesting,
  malformed metadata, or extension/type disagreement;
- a filename containing whitespace, quotes, leading dashes, newlines, control
  characters, Unicode normalization collisions, or shell metacharacters;
- symlink swap, destination collision, hard-link alias, input mutation, process
  kill, disk full, permission change, external-volume removal, or case-folding;
- a File Provider returning a temporary copy, dataless placeholder, delayed
  materialization, or conflict during writeback;
- extension termination, duplicate invocation, stale app upgrade, truncated
  stdout/JSON, or child process that outlives its UI;
- metadata leakage through JSON, stderr, notifications, temporary files,
  thumbnails, logs, crash reports, or journal-like state;
- accidental network access introduced by a framework entitlement or dependency.

### Out of scope, stated plainly

- malicious code already running as the same logged-in user;
- compromise in macOS, Image I/O, Finder, a File Provider, or system archive
  tools;
- visible-content detection, face/license-plate/document redaction, anonymity,
  steganography, perceptual fingerprinting, cloud copies created by the user's
  chosen storage provider, or deletion of prior copies/backups;
- proving a File Provider never uploads its own managed file. Études makes no
  network call, but selecting iCloud/Dropbox content invokes provider behavior
  outside Études.

### Controls

- no network entitlement, daemon, login item, telemetry SDK, auto-updater, or
  cloud API;
- absolute signed helper path, argv array, minimal environment, no shell,
  bounded output, closed inherited descriptors, cancellation/reaping;
- file identity checks before and after processing, no-follow opens, sibling
  temporary output, atomic no-replace placement, fsync, cleanup disclosure;
- no metadata values in logs/results; redacted paths follow existing rules;
- File Provider behavior is a separate test dimension and may be unproven;
- synthetic fixtures only.

## 9. Frozen experimental protocol

Every assertion reports `PASS`, `FAIL`, or `UNPROVEN`. Missing tools,
permissions, accounts, volumes, fixtures, or observability produce UNPROVEN,
never PASS. Each witness includes a positive control that would fail if the
property were broken.

### Gate F: Finder platform

Build a throwaway signed test bundle before connecting a real engine.

| ID | Frozen claim | Experiment / witness |
|---|---|---|
| F1 | A Quick Action receives one or many selected files and folders from arbitrary local Finder directories without losing ordering or path bytes. | Synthetic names including spaces, quotes, `$()`, leading dash, newline, NFC/NFD; adapter records length-prefixed argv in a scratch container. |
| F2 | No shell interpolation occurs. | A helper echoes NUL-delimited argv; canary filenames that would create a file if interpreted must create nothing. Process tree contains no shell/`osascript`. |
| F3 | The bundled Rust helper can access the selected input and intended output with only user-selected read/write rights and no network entitlement. | Test local file, selected folder, Desktop, Documents, Downloads, external APFS/exFAT, read-only volume. Inspect signed entitlements and sandbox denials. |
| F4 | Action output lands beside the selected input without silent overwrite. | Existing destination, case-fold/NFC collision, returned single file, returned directory; compare actual parent and identity. |
| F5 | File Provider inputs are materialized/coordinated honestly. | iCloud Drive plus one non-Apple provider if available: online, offline/dataless, eviction during read, conflict, sibling creation. Provider network activity is explicitly outside no-network claim. |
| F6 | Native UI represents exit 0/1/2/3 and malformed/truncated JSON without reinterpretation. | Fake helper emits every class, mixed batches, huge output, invalid UTF-8/JSON, early exit, signal death. Snapshot/accessibility assertions. |
| F7 | Cancellation and host termination leave recoverable engine state and no orphan helper. | Cancel before launch, during preflight, during write, after placement; kill extension/Finder; inspect process table, journal and partial outputs. |
| F8 | Install, enable, disable, upgrade and removal are reliable. | Clean account; disabled-by-default state; first Gatekeeper open; stable-ID upgrade; downgrade refusal; app removal; Finder relaunch; reboot; no login/background item. |
| F9 | “Show in Finder” selects only a reported successful output. | Refusal/error/mixed batch and deleted-output races. |

Gate F passes only on every supported macOS major/minor and architecture in the
release matrix. File Provider support may be explicitly omitted if F5 is
unproven or failed; the UI must then refuse those items rather than imply local
completion.

### Gate U: existing-tool equivalence

For a corpus of synthetic sweep/stash/unpack fixtures, invoke the CLI directly
and through the Action Extension. Compare normalized structured outcomes, exit
class, resulting tree, journal state, and disclosures. The only allowed Finder
differences are UI presentation and batch envelope. Any policy/destination or
refusal difference fails the gate.

**The read surface is part of the equivalence claim.** A sandbox entitlement
is a capability, not a behavior: `user-selected.read-write` says what macOS
permits, not what the code does. Each engine's read surface is its existing
contract — sweep reads names, sizes, and dates and never file contents;
unpack reads archives because that is its job; scrub reads image bytes by
stated contract — and the Finder path must exhibit the same surface as the
CLI path. Witness it, do not assume it: trace file-content opens (for
example, `fs_usage` or an `ESF`-based observer over the fixture corpus) on
both paths and compare. A sweep invocation via Finder that opens a file's
contents fails Gate U even if the resulting tree is identical, because the
privacy claim, not the sort, is the product.

### Gate S: `scrub`

| ID | Frozen claim | Independent observation |
|---|---|---|
| S1 | Original is byte-for-byte unchanged for success, refusal, error, cancel, and kill. | SHA-256 and stat identity before/after; hard-link alias checked too. |
| S2 | JPEG entropy-coded image data is byte-for-byte unchanged. | A minimal read-only JPEG marker/scan inspector, separate from mutation code, hashes all scan data and structural coding tables. |
| S3 | GPS, XMP/extended XMP, IPTC/Photoshop metadata, identifying EXIF, thumbnails, comments, dates, and unknown descriptive segments are absent. | At least two paths: independent marker/EXIF parser plus Image I/O property enumeration; test must fail on the dirty fixture. |
| S4 | Orientation values 1–8, including mirrored forms, render exactly equivalently. | Decode both with orientation applied and compare dimensions and raster hashes. |
| S5 | ICC/color rendering is preserved exactly. | Extract/hash ICC and compare tagged decode/raster; fixtures include sRGB and non-sRGB profiles. |
| S6 | Output is valid and no undeclared metadata remains. | Image I/O decode, independent parser, exact allowlist of remaining segment classes. |
| S7 | No overwrite, partial, unexpected sibling, or temp survives tested failures. | Collision, case-fold/NFC collision, permission change, kill, disk-full image, injected finalize/fsync/rename failures. |
| S8 | Input mutation is detected before placement. | Replace, truncate, append metadata, and modify scan data between read and final check. |
| S9 | Human, JSON, stderr, logs and notifications contain no planted private metadata value. | Unique GPS/owner/serial/date canaries searched bytewise and escaped; positive control emits one and must fail. |
| S10 | Runtime opens no socket and creates no persistent cache/thumbnail. | Existing proven sandbox plus filesystem trace of container, caches, destination and state directories. |
| S11 | Batch isolates inputs and summary precedence is honest. | Success/refusal/error/cancel permutations and duplicate inputs. |

### Synthetic fixtures

Generate every fixture under a unique temporary root from scripts or small
test-only programs. Do not commit opaque hostile media blobs when a readable
generator can produce them. Required set:

- JPEG with/without GPS; orientation 1–8; sRGB and non-sRGB ICC; EXIF thumbnail;
  IPTC; ordinary and extended XMP; body/lens serial, owner, maker note, software,
  date; comments; malformed metadata; near-64-KiB segments; progressive JPEG;
  unknown APP marker; misleading extension; polyglot where feasible;
- existing `-private` sibling; read-only input/destination; case-sensitive and
  case-insensitive volumes; NFC/NFD and control-character names; symlink;
  hard link; mid-read replacement; kill at every output phase; full volume;
- HEIC orientation/GPS/HDR gain map/depth/auxiliary/multiple-image fixtures for
  refusal and future research, even though v0.1 does not support them;
- PNG, TIFF, WebP, GIF/APNG, RAW-shaped/malformed input, Live Photo pair, PDF,
  and non-image files carrying photo extensions for refusal.

Fixture generators must record which properties they successfully created. A
fixture missing the intended GPS/profile/auxiliary item makes that test
UNPROVEN, not passed.

### Required witnesses

```sh
cargo test --all
scripts/no-network-test.sh
scripts/scrub-metadata-witness.sh
scripts/finder-action-test.sh
bash stress/run.sh
```

`scrub-metadata-witness.sh` and `finder-action-test.sh` must first falsify their
own checks with dirty/control fixtures. They print totals for passed, failed,
and unproven and fail if they make zero assertions. CI ratchets known failures
by scenario name, never only by count. Documentation numbers are added to
`check-claims.sh`; new user surfaces and their watchers are added to
`surfaces.toml`.

## 10. Frozen kill criteria

Kill the Action Extension route if any of these remains true after a bounded
prototype on the release OS matrix:

- selected-file access cannot be bridged to the existing engine without broad
  filesystem entitlements, a daemon, path copying that changes semantics, or a
  second policy implementation;
- Finder cannot place outputs beside arbitrary supported local selections or
  cannot refuse unsupported File Provider cases consistently;
- extension termination/cancellation can leave unreported partial work or an
  orphan helper after engine recovery is exercised;
- installation, enablement, stable-ID upgrade, or removal requires undocumented
  registry manipulation or routinely loses the actions;
- long operations cannot show truthful progress and cancellation without a
  persistent background component.

Kill `scrub` v0.1 if any of these is observed:

- removing the claimed JPEG metadata requires changing entropy-coded image data
  or breaks any orientation 1–8 or color-profile fixture, under BOTH candidate
  implementations — Image I/O failing alone does not kill v0.1 while the
  segment-level rewrite stands;
- the independent inspector cannot enumerate every remaining JPEG segment and
  falsify every removed-class claim;
- common JPEGs contain an open-ended set of unclassifiable segments such that
  fail-closed behavior makes the tool rarely usable;
- Image I/O or the chosen narrow parser cannot handle malformed input without
  partial output, unbounded resource use, or metadata-value leakage;
- “Remove Hidden Photo Metadata” plus the mandatory warning still causes study
  participants to believe visible details are redacted or the copy is anonymous;
- the product materially duplicates Finder's built-in Convert Image behavior
  once measured on the supported OS, without stronger witnessed preservation,
  refusal, and disclosure properties;
- maintenance requires chasing an open-ended metadata format list rather than
  enforcing a closed JPEG allowlist.

HEIC has independent kill criteria. Kill HEIC support, without killing JPEG, if
any single-image, orientation, ICC, HDR gain-map, depth/auxiliary, or multi-image
fixture cannot be classified and either preserved exactly or refused before
write; if output behavior varies across supported macOS releases; or if the
independent witness cannot observe all claimed auxiliary items.

## 11. Implementation sequence after this protocol

1. **Finder platform spike:** a throwaway Action Extension and inert argv helper
   for F1–F9. It touches only synthetic temporary files.
2. **Existing-engine vertical slice:** Open Archive Safely for one local ZIP,
   then batch, refusals, cancellation, volumes, File Providers, and lifecycle.
3. **Shared adapter:** only after the slice passes, factor typed inputs, bounded
   process execution, exit/result mapping, and native result UI.
4. **Put Aside / Bring Back:** no deadline default; selected-folder scope only.
5. **Sweep review:** build the minimum native review sheet and prove direct CLI
   equivalence. Do not ship an unreviewed organize action.
6. **JPEG research spike:** generate fixtures and implement independent
   inspectors before mutation. Test Image I/O losslessness and, if needed, a
   closed minimal container rewrite.
7. **`scrub` v0.1:** only if S1–S11 pass and no kill criterion fires. Add CLI
   first, then the Finder adapter as an equivalence client.
8. **HEIC research:** a separate protocol revision, never a release checkbox.

This order spends new policy risk only after the Finder transport is proven,
and keeps Finder a faithful front end to deterministic engines rather than a
second product hidden in an extension.
