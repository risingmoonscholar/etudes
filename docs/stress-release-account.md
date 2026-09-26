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
current `main` commit `ba7055206ca2f56bcbce628d753dba65427e327b`; the
release-gate unit tests also passed 5/5 there. Cargo used its local cache in
offline mode because this shell could not resolve crates.io. The exact current
check record is retained in
[`release-evidence/0.5.3-candidate-checks-ba705520-20260924.json`](release-evidence/0.5.3-candidate-checks-ba705520-20260924.json).
Earlier PR-head evidence remains in
[`release-evidence/0.5.3-candidate-checks-20260924.json`](release-evidence/0.5.3-candidate-checks-20260924.json).
These checks build and exercise candidate binaries locally; they do not verify
published tags. The latest Rust product-code commit is
`484feef76cd7b6eea450e65386d352b51af5769f`; later branch commits also add
release-gate scripts, tests, and Pages workflow wiring, alongside demo/evidence
and documentation updates.

After the demo and evidence corrections, the candidate install check passed at
local review commit `cc29019e2c44d0f6afb3737df404de9ee4251fd1`; all three tools
installed and passed their synthetic operation and recovery or refusal checks.
That check record contains 12 Pages-gate unit tests and an unverified remote tag
lookup because the shell could not resolve GitHub. The later Pages-gate result
is separate: at `af6c966`, its 14 unit tests passed and it rejected the current
page against live tags before upload. The claims check, all seven transcript
reproductions, and the focused NFC/NFD `EXDEV` fallback test also passed. The
candidate checks are retained in
[`release-evidence/0.5.3-candidate-checks-cc29019-20260924.json`](release-evidence/0.5.3-candidate-checks-cc29019-20260924.json).
The live Pages-gate result is retained in
[`release-evidence/0.5.3-pages-gate-20260924.json`](release-evidence/0.5.3-pages-gate-20260924.json).
The direct published-tag check failed as expected: all three proposed 0.5.3
tags are absent, so installing from those tags was not attempted. Its result is
retained in
[`release-evidence/0.5.3-published-tags-20260924.json`](release-evidence/0.5.3-published-tags-20260924.json).
The gate treats shared workspace tags only through `v0.5.1`; newer releases
must publish per-tool tags so one tool cannot be advanced by another tool's
release. Cursor's follow-up review found no remaining selector defect.

## Repository and release gates

PR #104 has merged to `main` as `12b47967bb08539058ecb22393ffab255b6c0876`.
The candidate work merged through PR #105 as
`ac1c106962e008f289c02093570cf61efde76b73`; this closes issue #103 with the
deterministic NFC/NFD fallback evidence above.

A read-only independent Cursor CLI review (Composer 2.5, Auto review, sandbox
enabled) examined product commit `484feef` and the local demo/evidence branch
against `main` at `ba70552`. It found no definite product defect in the macOS
fallback or progress code, and identified two definite demo-evidence wording
defects: a stale stress summary and a transcript field that called combined
stdout/stderr `stdout`. Those were corrected in local commits `cc5678a` and
`2795820`; a follow-up confirmed the stress totals and exit-code legend are
distinct. The review ran `cargo test --all` (278 passed, 1 ignored), but did not
rerun the stress suite or candidate-install checks. It is not Factory
certification, and it does not verify published tags. No Factory observer
result is claimed.

The 0.5.3 tags remain unpublished. Their installation and behavior cannot be
verified until those tags exist. The Sep. 24 tag check and the Sep. 25
independent recheck both found them absent; no release was made in this work.

The seven unproven platform cases are case-sensitive collision, cross-device
copy and mtime, cross-volume EXDEV, exFAT fallback, full volume, read-only
volume, and undo recovery after a volume hazard. They require a host capable of
creating and mounting the test disk images; they are not recorded as passes.

## Live Pages mismatch

On 2026-09-25, an independent read-only Cursor CLI check fetched the public
tags and hosted transcript manifest successfully. All three proposed 0.5.3
tags remain absent; the latest public versions are sweep 0.5.2, stash 0.5.2,
and unpack 0.5.1. However, the hosted Pages transcript manifest reports 0.5.3
for all three tools at capture commit `484feef`. The public demo therefore
advertises an unpublished candidate. This is not merely a pending release gate:
the Pages site already carries candidate version claims.

The local Pages gate rejects that manifest against the live tags, but the gate
and its workflow integration are only on the unpushed branch
`codex/0-5-3-release-demos`. `origin/main`'s Pages workflow does not run
`scripts/check-pages-release.py`. Consequently, the gate did not protect the
deployment already on the public site. The current independent findings and
tag evidence are retained in
[`../release-evidence/0.5.3-independent-release-review-20260925.json`](../release-evidence/0.5.3-independent-release-review-20260925.json).

## Current local rerun

On 2026-09-25, the candidate install and smoke checks passed again at checkout
`02378728c998438662d37672a84182ac981a316a`. The current changes are limited to
the demo page, GIF recordings, recording scripts, and capture theme; no Rust
product source changed. All three locally installed tools passed version,
synthetic operation, and recovery/refusal checks. The claims check, all seven
transcript reproductions, the 14 Pages-gate unit tests then present, and `cargo test --all`
also passed. The exact run is retained in
[`../release-evidence/0.5.3-candidate-checks-0237872-20260925.json`](../release-evidence/0.5.3-candidate-checks-0237872-20260925.json).

The ordinary candidate-check shell could not resolve GitHub, so its run does
not establish live tag state or verify installs from proposed public tags. The
separate Cursor CLI check did reach GitHub and confirmed the candidate tags are
absent and the public Pages manifest advertises 0.5.3. Tag-based installs
remain impossible until the user publishes tags. No publication was performed.


## Release-version Pages ratchet

After the live deployment mismatch was confirmed, the Pages workflow was tightened
in the current local worktree: only a per-tool stable GitHub Release or stable
promotion can trigger deployment. The job checks the release event and exact
per-tool version tag, checks out the release event's immutable commit, and
compares the page manifest with GitHub's explicitly stable Release records
before artifact upload. Demo-only
pushes, manual dispatches, and prereleases cannot deploy. Workflow tests cover
trigger, tag pinning, gate ordering, and prerelease exclusion. This wiring is
still only on the unpushed branch, so `origin/main` is not protected yet.
