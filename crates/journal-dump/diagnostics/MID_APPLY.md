# Mid-apply fault injection diagnosis

Base: `7879023317f9bffe62bc93e33fc50df2b0deb5a8` (0.5.4 candidate).
The two stress scenarios and all their assertions remain unchanged. This
change supplies a separate reproducible diagnostic and a proposal for review;
it does not claim that the existing timing probes are fixed.

Run as a non-root macOS user after `cargo build --release`:

```sh
python3 crates/journal-dump/diagnostics/mid_apply.py target/release/sweep
```

`observed-macos.jsonl` records the host result. The diagnostic uses private
500-file fixtures with distinct payloads, zero grace period, and a fresh
explicit journal key/state directory for each experiment. It sends SIGSTOP,
waits for `ps` to report stopped state, then verifies a nonempty partial move,
an initial journal, and a still-present target at group position 375. Since
apply writes its full initial journal before moving, this establishes that
the target belongs to the accepted apply, rather than just the earlier scan.
This diagnostic is separate from the stress verdict and does not modify it.

| Intervention | Apply exit | Moved | Survivors after exact undo |
|---|---:|---:|---:|
| Non-idempotent unlink of pending source while stopped | 3 | 375 | 499 |
| chmod 0555 while stopped, failed create proves user cannot write | 3 | 1 | 500 |
| Allow apply to finish between source-exists predicate and rm -f | 0 | 500 | 500 |
| chmod 0555 after apply finishes | 0 | 500 | 500 |

All four runs restored the expected original paths and bytes, excluding only
the intentionally deleted source. There were no phantom `already gone`
reports. The experiments establish these outcomes on this host and base;
they are not a universal absence-of-defects claim or proof of fallback-volume
behavior.

## Named CI evidence

* [PR 125 run 38041252902 attempt 1](https://github.com/risingmoonscholar/etudes/actions/runs/38041252902/attempts/1):
  40-01 claims deletion at 291ms, position 375, then reports exit 0, progress
  500/500, 500 files remaining, 500 moved and restored. Four assertions fail.
  All 500 generated files surviving contradicts an actual deletion.
* [PR 117 run 38031342788 attempt 4](https://github.com/risingmoonscholar/etudes/actions/runs/38031342788/attempts/4):
  40-06 reports success after chmod at 400ms. The witness establishes mode
  bits but never establishes a live unfinished apply at injection time.
  That log alone cannot prove when the last move happened. The late-chmod
  experiment reproduces exactly the condition permitting this false report.
* [PR 114 run 37757345691 attempt 1](https://github.com/risingmoonscholar/etudes/actions/runs/37757345691/attempts/1):
  earlier same deletion false report, progress and recovery all 500; the
  unchanged second attempt passed. A passing rerun is not a repair.

The source-exists check and idempotent rm are separate operations; rename
can happen between them. Both `rm -f` and the subsequent absence predicate
then succeed without deleting anything. Calibration on a different fixture
cannot close that gap. The permission probe similarly compares elapsed time
against another run instead of confirming unprocessed entries remain.

In `crates/etude-core/src/apply.rs`, the apply loop saves the complete plan
before moving, propagates `move_one` errors, and records a durable done entry
only after a successful move. On macOS its normal same-volume path is atomic
`renamex_np(RENAME_EXCL)`, not the unconditional link/unlink path described in
the permission scenario comment. Link/unlink remains a fallback on ENOTSUP;
these experiments do not claim to exercise that seam.

## Proposed witness integration (requires approval)

Extend the dev-only journal-dump watcher with an apply mode analogous to
`--watch-undo` from commit `6902603`. Use each trial's own fresh state directory
and key, discover its sweep journal, read durable Moved states, and SIGSTOP
its caller-supplied positive PID after early partial progress. Confirm OS
stopped state and reload the authenticated journal **after stopping**. A
count observed before SIGSTOP is insufficient: the process can advance
between observation and signal delivery.

Require the stopped journal to have all 500 planned entries and a nonempty
proper subset moved, with the position-375 deletion target still Planned.
Also confirm its source exists and destination does not. If the operation
finished before the stop, resume/reap as appropriate and retry a fresh trial;
never call that a successful injection. Any helper error after stopping must
resume the child or give the caller an explicit stopped-process cleanup path.
Bound observation to ten seconds, not the model run.

For 40-01, unlink the target while stopped with a non-idempotent operation,
confirm it is absent from source and destination, then SIGCONT and retain
all existing nonzero-exit, missing-count, journal/undo and survivor checks.

For 40-06, chmod the source while stopped, confirm 0555 and inability of the
same user to create a name, and confirm planned sources remain. SIGCONT,
restore mode before recovery, and retain all existing failure, duplicate and
no-loss assertions. Keep the current fixture size and target intent. Updating
the stale syscall comment needs the same witness review; no fallback-seam
coverage should be claimed without a supported fallback filesystem trial.

No witness integration is included here because approval is absent. Ten
clean CI jobs on this diagnostic-only change can measure existing behavior,
but cannot establish that the proposed integration stops the flake. Approval
and ten clean runs of the integrated witnesses remain necessary before any
claim of a completed repair.
