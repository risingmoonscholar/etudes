# Private, frictionless organization

November 10, 2026 · Discussion of Sweep's future direction

The goal is to help people organize core macOS folders without asking them to trust a cloud agent with their files. Screenshots containing reminders, bugs, personal information, and other material were one example, not the complete use case. Downloads and Desktop may contain anything: Taildrop handoffs between running agents, JSONL, logs, JSON objects, scripts, HTML, and files whose purpose cannot be inferred from their extension. macOS conventions are finite; the contents of these folders are not.

Sweep currently favors content-blind decisions, safety holds, a plan before mutation, and undo. This limits semantic organization. A possible future direction is optional local analysis that takes a person's organizing goal and produces a locally reviewable plan. The cloud agent could work with bounded results such as counts and opaque group identifiers, while original bytes and content-derived descriptions remain local by default. Even descriptions and filenames can reveal private information; redaction is fallible. Sharing selected material with a cloud agent would be a separate, explicit choice. A local model introduces quality, availability, and resource costs, and an agent unable to see details cannot independently reason about those details.

Separate whether something is safe to touch from where it might belong. Recent changes, partial transfers, project markers, related files, locks, and user-defined exclusions are useful signals, not proof that a file is idle. In particular, an old Taildrop handoff or log can still be needed by a running agent. A configurable hands-off area or convention could help. Related files should be kept together where evidence supports it. Uncertain items should stay put, with understandable counts and a practical way to review and correct choices in batches. Undo helps recover a move but cannot necessarily repair an interrupted workflow.

Start by investigating better activity and relationship handling, user-controlled exclusions, and learnable routing before committing to local AI or a wide range of parsers. Test across mixed, unfamiliar folders: measure useful grouping, items left uncertain, disclosure through outputs, and the effort required to correct and undo decisions. Maintain the content-blind default unless the user chooses otherwise.

Sweep alone cannot prevent an agent with unrestricted shell or filesystem access from reading files. A privacy boundary for cloud agents requires an appropriately constrained integration or OS-enforced sandbox, including attention to output, logs, temporary files, and metadata. A plan digest protects plan integrity, not confidentiality or agent access. These are design ideas and tradeoffs, not claims about features already implemented.

## Friction review of the 0.5.5 candidate

Reviewed at commit `4443b9e`; these are workflow findings, not claims that the release violates its contract.

1. **The obvious apply command makes a fresh plan.** A preview followed by `sweep apply PATH --yes` can select different files in a changing folder. Applying exactly what was previewed requires exporting a plan and carrying its digest into a separate command. Make exact-plan application the easy path, with a clear recovery when a plan becomes stale. See `docs/plan-binding.md`.
2. **Age does not establish inactivity.** The default one-day modification-time grace period cannot identify an older handoff or log still needed by a process. Persistent user-controlled exclusions and per-file leave-alone choices would help; neither proves that all active files are detected. See `crates/etude-core/src/scan.rs`.
3. **Review works at group granularity.** One important file in a group can force a person to skip the whole group or use the exported-plan proposal workflow. Local per-file exclusions and batch selection would make conservative choices easier. See `crates/sweep-cli/src/review.rs`.
4. **Custom routing requires substantial ceremony.** Built-in groups take precedence over `--map`. Individual proposals require an exported plan, a JSON proposal, another export and digest, and an existing destination. Existing-folder profiles are extension-based. A local selection and destination flow could bridge this gap. See `docs/sweep.md` and `crates/etude-core/src/plan.rs`.
5. **Content blindness does not make metadata private.** Exported item details include names and sizes; plan artifacts contain selected paths. An agent integration that keeps names local would need a more restricted interface and a local review surface. See `docs/plan-binding.md`.

For the release in progress, make the preview/apply distinction and active-file limitation unmistakable in user-facing instructions. Per-file review and a privacy-preserving agent interface are broader follow-on work. This was a targeted code and documentation review, not a full release audit or test run.
