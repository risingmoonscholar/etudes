# Pack disclosure and archive protocol

Version 1 is a proposal for a future `pack` capability. It defines what an archive operation would include, refuse, validate, and preserve. This protocol and its fixtures do not implement or establish a working pack tool.

## Scope and recipient policy

Pack creates a new archive from an explicitly selected source tree for an explicitly named recipient or destination class. The caller chooses a policy before preflight. The policy records sensitive-name handling, in-flight-download behavior, archive format and retained metadata streams, and a maximum output size. Missing, unknown, or contradictory policy is refusal. Nothing is inferred from the destination, environment, or agent identity.

The disclosure exclusion set comes from `etude_core::classify::sensitive`; pack must not duplicate that taxonomy. A conformance check compares pack's exclusions with every entry sweep holds as `LooksPersonal` and fails if pack excludes less. This governs disclosure, not sweep's movement policy: a complete project or package may be included but must never be split.

## Preflight and archive shape

Preflight enumerates a stable source manifest and detects case-insensitive and Unicode-normalization collisions under the target extraction model, reserved names, traversal or absolute paths, and symlinks escaping the selected root. Unsupported nodes and unrepresentable paths cause refusal before publication. Packages and bundles are opaque trees: include the complete package or refuse. Recently modified files remain eligible; pack does not inherit sweep's grace window. An in-flight download is always refused and named because its bytes may be partial or corrupt.

MacOS litter, AppleDouble files, resource forks, extended attributes, and other metadata require an explicit per-format policy. If a required data stream cannot be retained for the selected format or recipient, refuse or report authorized loss before creating the final archive. Preflight calculates a conservative size bound including archive overhead, checks available space, and enforces the maximum while streaming. A violation aborts only private staging. If the format cannot be bounded safely, refuse.

## Publication and source preservation

Write only to a new private staging object. Never overwrite, append to, or delete an existing destination. Before publication, independently read the closed archive, verify its directory and member manifest against the plan, and confirm source identity and relevant metadata have not changed. Publish atomically with exclusive no-replace semantics; otherwise refuse.

The source tree is read-only from pack's perspective. Never rename, unlink, modify, normalize, or clean up source entries. After publication reopen and verify the output. A failed post-publication check retains it and reports `incomplete`; it does not delete the output as rollback. Cleanup removes only staging created and still identified as owned.

Default receipts use opaque item identifiers and redact unrelated source paths. A refusal may identify the in-flight entry needed for correction. Never report raw contents, secret values, or unsolicited fingerprints.

## Outcomes and evidence

A future implementation should distinguish `done`, `refused`, `error`, and `incomplete` through the established result envelope and a separately versioned capability declaration. `done` requires policy compliance, source-preservation checks, and independent readback. The fixture manifest lists expected outcomes only; each remains unmeasured until an implementation runs it. A protocol proposal is not runtime evidence.
