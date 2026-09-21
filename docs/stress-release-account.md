# Stress-suite release account

**Release status: validation pending.** This account is ready to accompany the
stress-suite changes once the remaining platform and independent-review gates
have completed.

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
| Fast | 621 passed, 0 failed, 1 unproven | NFC/NFD cross-device fallback unavailable on this host |
| Load | 103 passed, 0 failed, 0 unproven | 9,999-file apply: 76.69 s; undo: 58.73 s |
| Platform | 58 passed, 0 failed, 0 unproven | All eight platform scenarios ran on this macOS host |

The 1,100-file smoke run measured a 5.17 s apply and 5.06 s undo. The full
9,999-file run is the cost evidence: it took 76.69 s to apply and 58.73 s to
undo. It also had a higher per-file apply cost (7.67 ms versus 4.70 ms), so the
smoke result is retained only to prove the measurement path.

The disk-image contracts now cover case-sensitive and cross-device behavior,
exFAT fallback, full/read-only volumes, and undo on a volume hazard. The
separate fast-tier NFC/NFD cross-device copy fallback remains unproven because
this host cannot force that collision ordering.

## Release gates

1. Exercise the NFC/NFD cross-device fallback on a host where its collision
   ordering can be forced; retain the result bundle.
2. Run CI and obtain a completed independent factory observer review after the
   factory-core authentication/review-integrity repair is released.
3. Publish this account only when both gates are successful.
