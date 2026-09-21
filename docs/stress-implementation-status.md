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

The current fast-tier run at `22c2741` completed on this host with **627
passed, 0 failed, 1 unproven**; its retained result bundle is
`20260921T224325Z-5184`. The unproven assertion was the NFC/NFD cross-device
copy fallback, which this host could not exercise. The result explicitly named
the 9 load and 8 platform contracts as not run.

[Issue #103](https://github.com/risingmoonscholar/etudes/issues/103) tracks the
remaining cross-device Unicode witness with a falsifiable closure condition.

The load tier completed with **103 passed, 0 failed, 0 unproven**. Its
9,999-file timing case measured: build 4.44 s, plan 2.00 s, apply 76.69 s,
undo 58.73 s, verification 0.30 s, cleanup 0.28 s. That is the load evidence;
the earlier 1,100-file run only validated the measurement path. Its 5.17 s
apply and 5.06 s undo figures are not linearly comparable to the full run:
the full fixture took about 7.67 ms per file to apply versus 4.70 ms per file
in the smoke run. The release record therefore uses the full run for cost
claims and retains the smoke run only as a measurement-path check.

The platform tier completed with **58 passed, 0 failed, 0 unproven**. It ran
all eight platform scenarios on this host, including real APFS/exFAT images,
case-sensitive media, full and read-only volumes, descriptor limits, and
cross-device interruption. Its retained result bundle is
`20260921T212153Z-66061`.

## Guide checkpoint audit

| Guide stage | Evidence in this branch |
| --- | --- |
| Inventory | `stress/catalog.json` has one disposition, tier, capability, and contract for each of the 46 tracked scenarios. `scripts/check-stress-catalog.py` rejects missing, duplicate, or invalid rows. |
| Reporting | `stress/lib.sh` records pass, fail, unproven, and completion separately. `85-scenario-exit-status.sh` falsifies hidden failures, aborted cases, signal death, and all-unproven cases. |
| Shared helpers | `snapshot.py` records paths, kinds, modes, payload digests, and link targets. `bounded.py` owns and reaps a process group. Failed cases retain their transcript, assertions, process record, and snapshots. |
| Movement and recovery | Desktop and interrupted-undo scenarios assert real changes, protected paths, and exact recovery. Local no-op, dropped-movement, and false-recovery mutants are rejected. |
| Races | The repaired race scenarios record their intervention before judging safety; missed timing windows report unproven. Their waits are bounded and every runner case has an outer per-case deadline. |
| Cost | Load cases use contract-sized fixtures and phase timing. The 9,999-file run supplies the load evidence; the earlier 1,100-file smoke run only exercises the measurement path. |
| Consolidation | Mapping and document-scope behavior are separate scenario contracts; grace timing is relative; provider-plan checks compare manifests. |
| Scheduling | The catalog drives fast, load, and platform tiers. The CI selection self-test distinguishes an explicitly inapplicable docs-only PR from a source change and from scheduled/manual full runs. |
| Maintenance | `CONTRIBUTING.md` carries the six review questions. Result bundles identify revision, runner, skipped contracts, failures, unproven work, and slowest cases. |

## Coverage that remains host-dependent

Seven platform scenarios still need a host that can create real disk images:
case-sensitive collision, cross-device copy and rename fallback, exFAT
fallback, full-volume behavior, read-only media, and undo recovery on a volume
hazard. The separate fast-tier NFC/NFD cross-device copy fallback is likewise
unproven. `hdiutil create` failed with “Device not configured,” and `diskutil`
reported that its DiskManagement framework is unavailable on this host, so the
seven disk-image cases were recorded as `unproven`. The descriptor-limit
contract did run and passed. Missing capability is never counted as a pass.

Independent review is also not claimed here. Factory-core recovery is outside
this change; release certification or publication still requires a successful
observer, not an authentication-error surrogate.
