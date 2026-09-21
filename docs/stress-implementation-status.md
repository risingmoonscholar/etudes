# Stress-suite implementation status

This is the implementation record for the stress-test guide. It is evidence
from this branch, not a release certification or independent-review result.

## What changed

| Area | Contract now enforced |
| --- | --- |
| Result reporting | Every case has an explicit completion record and a pass, fail, or unproven verdict. Result bundles include the revision, runner identity, case contract, capability, tier, duration, child exit, failure-evidence path, and five slowest cases. |
| Movement and recovery | Desktop, allowed-sync, package, state-directory, hostile-name, and interrupted-undo cases compare filesystem manifests rather than only counts or output. Harness mutants prove the historical no-op Desktop and undo executables, a mover that drops work, and a recovery that falsely reports success are rejected. Snapshot mutants prove same-size corruption, a missing/replaced entry, and a changed symlink target are visible. |
| Interrupted extraction | A killed unpack supervisor never publishes a partial requested target; a retry publishes the exact archive after private staging completes. |
| Interrupted stash/pop | Killed stash and pop operations each leave a witnessed partial state with every payload present; the next pop restores the exact original tree. |
| Same-root contention | Two live applies on one root produce one winner and one resumable loser; every payload remains present and one undo restores the exact original tree. |
| Races | Source deletion, destination collision, symlink replacement, growing writers, directory blockers, and permission changes record an independent intervention witness before judging the result. |
| Cost | The fixed 1.5 GiB cross-volume payload, 50k duplicate cap fixture, 1,450-file group fixture, and repeated signal timing guesses were replaced by contract-sized or bounded probes. |
| Scheduling | The catalog assigns all 46 scenarios to `fast`, `load`, or `platform`; CI runs the fast tier on relevant pull requests and the full tier on scheduled/manual work. The stress job has a 45-minute outer deadline, and every scenario runs in an owned process group with a configurable five-minute default deadline; timeout evidence records the reaped group. A tiered result names every catalogued contract it did not run. The CI scope rule self-tests that documentation-only pull requests visibly skip, while source changes and scheduled/manual events execute the suite. |
| Evidence | Failure-only transcripts, assertion records, process records, and generated filesystem manifests are retained; CI uploads them with the generated result bundle. |

## Local evidence

The final fast-tier run at `d404fbf` completed on this host with **611 passed,
0 failed, 1 unproven**. The unproven assertion was the NFC/NFD cross-device
copy fallback, which this host could not exercise. The run explicitly reported
the 7 load and 8 platform contracts as not run, rather than treating them as
passes. Targeted checks also cover the updated harness, sync-stub contract,
interruption cases, grace-window case, group/cap consolidations, and the
1,100-file timing smoke run.

The timing smoke run on Darwin/arm64 measured 1,100 files: build 0.05 s, plan
0.12 s, apply 5.17 s, undo 5.06 s, verification 0.06 s, cleanup 0.03 s. The
default load scenario remains 9,999 files; the smoke setting validates the
measurement path without presenting itself as the load result.

## Guide checkpoint audit

| Guide stage | Evidence in this branch |
| --- | --- |
| Inventory | `stress/catalog.json` has one disposition, tier, capability, and contract for each of the 46 tracked scenarios. `scripts/check-stress-catalog.py` rejects missing, duplicate, or invalid rows. |
| Reporting | `stress/lib.sh` records pass, fail, unproven, and completion separately. `85-scenario-exit-status.sh` falsifies hidden failures, aborted cases, signal death, and all-unproven cases. |
| Shared helpers | `snapshot.py` records paths, kinds, modes, payload digests, and link targets. `bounded.py` owns and reaps a process group. Failed cases retain their transcript, assertions, process record, and snapshots. |
| Movement and recovery | Desktop and interrupted-undo scenarios assert real changes, protected paths, and exact recovery. Local no-op, dropped-movement, and false-recovery mutants are rejected. |
| Races | The repaired race scenarios record their intervention before judging safety; missed timing windows report unproven. Their waits are bounded and every runner case has an outer per-case deadline. |
| Cost | Load cases use contract-sized fixtures and phase timing. The 1,100-file smoke run exercises the 9,999-file measurement path without claiming it is the load result. |
| Consolidation | Mapping and document-scope behavior are separate scenario contracts; grace timing is relative; provider-plan checks compare manifests. |
| Scheduling | The catalog drives fast, load, and platform tiers. The CI selection self-test distinguishes an explicitly inapplicable docs-only PR from a source change and from scheduled/manual full runs. |
| Maintenance | `CONTRIBUTING.md` carries the six review questions. Result bundles identify revision, runner, skipped contracts, failures, unproven work, and slowest cases. |

## Coverage that remains host-dependent

Platform scenarios need real APFS/exFAT images, read-only media, full-volume
conditions, case-sensitive APFS, or descriptor-limit support. A missing
capability reports `unproven`; it is not counted as a pass. The local
cross-volume run could not create its APFS image, so it remains unproven on
this host.

Independent review is also not claimed here. Factory-core recovery is outside
this change; release certification or publication still requires a successful
observer, not an authentication-error surrogate.
