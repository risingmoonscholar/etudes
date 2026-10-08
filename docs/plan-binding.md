# Applying an observed plan

Sweep and stash can explicitly export a bounded plan and later apply those original observations. A changed tree is refused with `replan required`; replay does not scan a replacement proposal and apply it silently.

```sh
sweep ~/Desktop --since 0 --export-plan ~/desktop.plan --json
sweep apply --plan ~/desktop.plan --plan-digest SHA256_FROM_EXPORT --yes --json

stash ~/Desktop --for 3d --export-plan ~/holding.plan --json
stash --plan ~/holding.plan --plan-digest SHA256_FROM_EXPORT --json
```

Export is a preview: it creates only the requested artifact, outside the selected tree, and moves nothing. The digest is in `details.binding.plan_digest`. Keep the digest from the original response separately from the plan file. Import requires that digest, verifies the artifact checksum, and rejects truncated, oversized, unknown-version or altered files. Existing export files are never overwritten. Filesystems that cannot represent mode 0600 are refused.

The private binary format is version 1, capped at 64 MiB with bounded records and 4,096-byte path/string fields. It preserves raw Unix bytes for selected member paths. The artifact contains selected paths and filesystem metadata; protect it like a directory listing. Held filenames, ancestor markers and other snapshot names are SHA256 commitments. There are no payload contents, keys or environment values. SHA256 commits the metadata, root, proposal, context and acceptance/rename choices; it does not authenticate the person who exported the file.

Exported-plan application checks the root's device/inode identity, each observed file's device/inode, size, type, mode and nanosecond modification/change timestamps, directory-name observations and relevant ancestor project markers. It also checks the actual CLI tool version, scheme/profile configuration and a stable capability-declaration digest excluding only the volatile contract-query operation identifier. Imported planning configuration and stash's original absolute holding deadline are frozen. Changing mapping, scan depth, grace, content-inspection or sync choices requires a new export.

Sweep may accept all exported groups or select one with `--only`. Interactive review may accept, skip or rename groups. Those decisions recompute the final plan digest from the original immutable observations. They never refresh the snapshot. If the tree changes during a prompt, the eventual mutation is refused. All public engine apply entrypoints require a bound plan and independently supplied current context; read-only display proposals cannot become mutable plans.

Direct `sweep apply PATH --yes` makes a fresh proposal and validates root identity, entry identities, permissions, layout and project markers before moving. An open writer may change an existing regular file's size and timestamps: same-device rename preserves its inode and open descriptor. Exported replay and interactive review retain strict size and timestamp checks. Ordinary `stash PATH` also retains strict metadata validation. To apply an earlier preview, use explicit export and replay; direct apply never substitutes for that replay.

A direct attempt that loses a planning race retains a sealed empty journal record when journalling is enabled. The refusal states that this attempt moved nothing and that existing journals remain recoverable. Exported-plan refusal creates no journal or case-folding probe. A stale exported plan remains stale; the caller must explicitly replan.

The snapshot observes the same depth, whole-unit policy, protected-directory exclusions, package opacity and marker probes used during planning. It binds observed metadata, not unobserved package or directory contents. Unreadable held directories are opaque: their observed directory identity and permissions remain bound, and none of their unobserved children can become selected sources. Safe sibling operations remain available. Planning adds no journal edge-hash/content reads. Changes outside that declared observation scope remain unproven. Nanosecond metadata checks can conservatively reject harmless changes; they are not cryptographic payload verification or an OS filesystem lock. Existing destination collision, overwrite refusal and crash-recovery checks still apply after binding validation.

Core fixtures and end-to-end CLI replay tests cover changed, added and replaced files, new project markers, root replacement, stale contexts, modified artifacts, original-digest enforcement, selection/rename fidelity, and unchanged replay followed by undo. Existing collision and injected journal/move failure tests retain their original assertions.
