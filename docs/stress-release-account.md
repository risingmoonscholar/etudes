# Stress-suite release account

**Release status: candidate validation complete; certification and publication
are pending.** This account records the unpublished 0.5.3 candidate. No 0.5.3
tag or release has been created.

## Candidate changes

The candidate prevents macOS cross-device fallback from overwriting a
normalization-equivalent destination, with a deterministic EXDEV test against
the host's real APFS NFC/NFD alias. It also reports bounded progress for long
apply, undo, stash, and stash-pop operations on stderr while keeping JSON
stdout clean. Small operations remain quiet. The 9,999-file case records the
actual cost and verifies that progress reaches completion; it does not claim a
performance optimization.

## Current evidence

The latest full stress run completed on Apple Silicon macOS at candidate commit
`67363b3dc3eb4038d772c0139ec601f855f33961` (run
`20260924T095419Z-86516`). Its per-scenario summary is retained in
[`release-evidence/0.5.3-stress-20260924.json`](release-evidence/0.5.3-stress-20260924.json).

| Tier | Passed | Failed | Unproven | Explanation |
| --- | ---: | ---: | ---: | --- |
| Fast | 629 | 0 | 0 | Includes the deterministic NFC/NFD fallback witness. |
| Load | 105 | 0 | 0 | The 9,999-file apply and undo completed with progress through the final item. |
| Platform | 10 | 0 | 7 | Disk-image cases could not create the required images with `hdiutil` on this host. |
| **Total** | **744** | **0** | **7** | No failed cases; seven platform capabilities remain unproven here. |

The focused NFC/NFD case passed 8 assertions. It injects EXDEV at the rename
boundary while using the real APFS normalization alias; the exclusive copy
fails on the occupied destination and preserves the source and existing file.
This is a deterministic witness for [issue #103](https://github.com/risingmoonscholar/etudes/issues/103),
not evidence that this host can mount a separate filesystem image.

The focused 9,999-file run (`20260924T095906Z-45093`) passed 13 assertions:
build 0.494 s, plan 2.121 s, apply 47.340 s, undo 45.718 s, verify 0.347 s,
cleanup 0.294 s. The full run's scale case measured 88.424 s including its
scenario setup and checks. These measurements establish visible progress and
current latency; journal optimization remains a separate, profile-led question
tracked by [issue #12](https://github.com/risingmoonscholar/etudes/issues/12).

Local candidate installation checks passed for sweep, stash, and unpack at
PR head `1ef5f5e2a41ef2eabd6a271f3607ebaf639ea3d3`; the release-gate unit tests
also passed 5/5 at that revision. The exact check record is retained in
[`release-evidence/0.5.3-candidate-checks-20260924.json`](release-evidence/0.5.3-candidate-checks-20260924.json).
These checks build and exercise the candidate locally; they do not verify
published tags. The latest product-code commit is
`484feef76cd7b6eea450e65386d352b51af5769f`; later commits update demos,
release evidence, and documentation only.

## Repository and release gates

PR #104 has merged to `main` as `12b47967bb08539058ecb22393ffab255b6c0876`.
The 0.5.3 candidate work is on a separate branch and still requires PR review
and merge. No independent factory observer result is claimed. If required
review or certification is unavailable, hold certification and publication.

The seven unproven platform cases are case-sensitive collision, cross-device
copy and mtime, cross-volume EXDEV, exFAT fallback, full volume, read-only
volume, and undo recovery after a volume hazard. They require a host capable of
creating and mounting the test disk images; they are not recorded as passes.
