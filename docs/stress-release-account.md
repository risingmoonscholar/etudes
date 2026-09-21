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

## Current local evidence

| Tier | Result | Material limit |
| --- | --- | --- |
| Fast | 621 passed, 0 failed, 1 unproven | NFC/NFD cross-device fallback unavailable on this host |
| Load | 103 passed, 0 failed, 0 unproven | 9,999-file apply: 76.69 s; undo: 58.73 s |
| Platform | 10 passed, 7 unproven | DiskManagement is unavailable here; `hdiutil create` reports “Device not configured” |

The seven unproven platform contracts cover case-sensitive and cross-device
behavior, exFAT fallback, full/read-only volumes, and undo on a volume hazard.
They must run on a macOS host with working DiskManagement and disk-image
creation before this account can represent a release certification.

## Release gates

1. Run the seven disk-image platform contracts on a host where image creation
   works and retain the result bundle.
2. Run CI and obtain a completed independent factory observer review after the
   factory-core authentication/review-integrity repair is released.
3. Publish this account only when both gates are successful.
