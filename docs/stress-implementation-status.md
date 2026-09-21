# Stress-suite implementation status

This is the implementation record for the stress-test guide. It is evidence
from this branch, not a release certification or independent-review result.

## What changed

| Area | Contract now enforced |
| --- | --- |
| Result reporting | Every case has an explicit completion record and a pass, fail, or unproven verdict. Result bundles include the revision, runner identity, case contract, capability, tier, duration, child exit, failure-evidence path, and five slowest cases. |
| Movement and recovery | Desktop, allowed-sync, package, state-directory, hostile-name, and interrupted-undo cases compare filesystem manifests rather than only counts or output. Harness mutants prove the historical no-op Desktop and undo executables, a mover that drops work, and a recovery that falsely reports success are rejected. Snapshot mutants prove same-size corruption, a missing/replaced entry, and a changed symlink target are visible. |
| Races | Source deletion, destination collision, symlink replacement, growing writers, directory blockers, and permission changes record an independent intervention witness before judging the result. |
| Cost | The fixed 1.5 GiB cross-volume payload, 50k duplicate cap fixture, 1,450-file group fixture, and repeated signal timing guesses were replaced by contract-sized or bounded probes. |
| Scheduling | The catalog assigns all 43 scenarios to `fast`, `load`, or `platform`; CI runs the fast tier on relevant pull requests and the full tier on scheduled/manual work. The stress job has a 45-minute outer deadline, and every scenario runs in an owned process group with a configurable five-minute default deadline; timeout evidence records the reaped group. A tiered result names every catalogued contract it did not run. |
| Evidence | Failure-only transcripts and assertion records are retained; CI uploads them with the generated result bundle. |

## Local evidence

The fast tier previously completed on this host with **582 passed, 0 failed,
1 unproven**. The unproven assertion was the NFC/NFD collision fixture, which
this filesystem could not represent. Targeted checks after later changes cover
the updated harness, sync-stub contract, interruption cases, grace-window
case, group/cap consolidations, and the 1,100-file timing smoke run.

The timing smoke run on Darwin/arm64 measured 1,100 files: build 0.05 s, plan
0.12 s, apply 5.17 s, undo 5.06 s, verification 0.06 s, cleanup 0.03 s. The
default load scenario remains 9,999 files; the smoke setting validates the
measurement path without presenting itself as the load result.

## Coverage that remains host-dependent

Platform scenarios need real APFS/exFAT images, read-only media, full-volume
conditions, case-sensitive APFS, or descriptor-limit support. A missing
capability reports `unproven`; it is not counted as a pass. The local
cross-volume run could not create its APFS image, so it remains unproven on
this host.

Independent review is also not claimed here. Factory-core recovery is outside
this change; release certification or publication still requires a successful
observer, not an authentication-error surrogate.
