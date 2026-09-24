# Stress-suite release account

**Release status: validation pending.** The local fast, load, and platform
tiers have completed, and CI has passed. Review and merge remain pending.

## Delivered behavior

The suite now contains 46 catalogued scenarios with an explicit contract,
capability, and tier. It records a verdict for every executed case, retains
failure evidence, measures phase timings, and gives each case a bounded,
reaped process group. The suite also proves that no-op movement, dropped work,
false recovery, snapshot corruption, interrupted extraction, interrupted
stash/pop, and same-root contention do not silently look successful.

## Coverage shape

The suite replaced oversized, duplicate, or weakly observed probes with
contract-sized fixtures: a 1.5 GiB copy payload, a 50k duplicate-cap fixture,
and a 1,450-file group fixture were consolidated into bounded cases. The
remaining 46 scenarios each cover a distinct behavior, while the catalog makes
their tier and required capability explicit.

## Current local evidence

| Tier | Result | Material limit |
| --- | --- | --- |
| Fast | 627 passed, 0 failed, 1 unproven | NFC/NFD cross-device fallback unavailable on this host |
| Load | 103 passed, 0 failed, 0 unproven | 9,999-file apply: 44.38 s; undo: 45.50 s |
| Platform | 58 passed, 0 failed, 0 unproven | All eight platform scenarios ran on this macOS host |

The full 9,999-file run is the cost evidence: it took 44.38 s to apply and
45.50 s to undo. The earlier 1,100-file smoke run only exercised the
measurement path and is not used for cost claims.

The disk-image contracts now cover case-sensitive and cross-device behavior,
exFAT fallback, full/read-only volumes, and undo on a volume hazard. The
separate fast-tier NFC/NFD cross-device copy fallback remains unproven because
this host cannot force that collision ordering. [Issue #103](https://github.com/risingmoonscholar/etudes/issues/103)
tracks the required real-filesystem witness or a deterministic replacement.

## Release gates

1. Keep the NFC/NFD fallback limitation explicit and tracked in issue #103;
   this run records it as unproven, not as a pass.
2. Keep CI green and complete review and merge. No independent factory
   observer result is claimed by this account.
3. Publish this account with the final candidate evidence after those gates
   succeed.
