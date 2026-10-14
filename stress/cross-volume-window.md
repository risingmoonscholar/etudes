# Cross-volume interruption window (overnight item 3b)

Base: `aba3ffc29a629deae17397cb06fe78bcb7829ef0`. Only the stress witness changes.

The old observer slept for a requested 1 ms, but each existing-destination poll
also forked `stat` through a command substitution. After seeing a partial file
it sent SIGSTOP, then forked `ps | tr` to confirm the stopped process. A delayed
size observation can therefore consume most of a short copy's remaining window.
A completed destination after stop confirmation is correctly treated as a miss.

Measurements on the local Apple M4 Max, in two batches with varying concurrent
overnight load:

| Measurement | First batch (ms) | Repeat batch (ms) |
| --- | --- | --- |
| 32 MiB actual sweep copy, first nonempty destination to full size | 142.866, 113.815, 239.703 | 5.914, 113.866, 163.483 |
| 96 MiB actual sweep copy, first nonempty destination to full size | 302.189, 532.341, 405.128 | 210.191, 306.599, 304.583 |
| 256 MiB actual sweep copy, first nonempty destination to full size | 767.411, 2153.956, 1402.587 | 545.127, 1054.903, 948.309 |
| Old poll period, mean over 100 iterations with an existing file | 19.423, 16.228, 14.069 | 18.967, 13.021, 11.175 |
| Old `ps | tr` confirmation, mean over 100 invocations | 8.097, 9.603, 9.368 | 4.910, 2.862, 2.911 |

The copy measurements used the release sweep binary and a real temporary APFS
image, with a passive observer using `stat` and a requested 100 microsecond
sleep. Each completed apply was undone before the next measurement. Poll
measurements used the old shell loop's commands against a stable fixture. These
are local measurements under load, not timings from the cited CI runs. The
fastest 32 MiB copy window (5.914 ms) is shorter than one old poll period
(11.175–19.423 ms). Stop confirmation latency is not itself time spent copying
if SIGSTOP has already taken effect; the key improvement is observing the
partial size and sending SIGSTOP promptly.

The baseline still escalates through **32, 96, and 256 MiB**. Every restored
miss emits its own `unproven` line, even if a later size proves the property.
Thus a 32 MiB `unproven` line alone does not establish that escalation failed.
There is no early exit on an ordinary restored miss: both probes continue to
the next size. A real assertion failure during restoration does exit.

The new observer is ready before apply starts, receives the exact PID through
a FIFO, and polls with in-process `os.stat` plus a requested 100 microsecond
sleep. It sends SIGSTOP directly on the first partial-size observation, then
uses `ps` to confirm the stopped state. The shell retains its own stopped-size
and source-integrity checks before SIGKILL, verifies SIGKILL's wait status,
checks source bytes, checks retry refusal and unchanged destination bytes, and
checks source bytes after the refusal. The removed-source control retains the
same integrity predicate. Misses still restore, report `unproven`, and escalate;
a final 512 MiB attempt provides additional headroom without a 1 GiB payload.
The scenario logs observed poll intervals, stop-confirmation latency, and sizes
for both the negative control and positive probe.

## Validation

The ten requested `bash stress/run.sh 60-cross-volume-exdev </dev/null` runs
all exited 0: **10 proven, 0 unproven**, with 15 passed assertions and no failed
or unproven assertions per run. Both the positive probe and removed-source
control caught the 32 MiB copy in every run. New observed mean poll intervals
were 0.168–0.491 ms; stop confirmation took 3.296–59.275 ms under load, while
the stopped destination still remained partial. The old measured polling
period was at least 22 times the new observer's slowest measured mean.

Two additional temporary scenario copies exercised the restoration paths without
modifying the committed witness or sweep. One deliberately resumed the control
at 32/96 MiB, reported both misses as `unproven`, then proved both control and
positive probe at 256 MiB. The second deliberately resumed the positive probes
at 32/96/256 MiB after
the removed-source control passed at 32 MiB. All three misses restored and
reported `unproven`, and the 512 MiB fallback then proved SIGKILL with every
source and retry assertion passing. Both validations exited 0.

The initial sandbox attempts could not create the APFS image, create the
suite's raw-name fixture, or obtain the online baseline for network denial.
Those checks were rerun with the required permissions. The initial tool
contract attempt preceded the debug binary build; building the binaries and
rerunning resolved it. Shared stress-lock refusals (exit 3) were retried after
other worktrees released their lock, without disturbing their processes.

All requested commands received closed stdin (`</dev/null`):

| Command | Initial exit where different | Final exit |
| --- | --- | --- |
| `cargo fmt --all --check` | — | 0 |
| `cargo clippy --all-targets --all-features -- -D warnings` | — | 0 |
| `cargo test --all` | 101 (sandbox denied raw-name fixture) | 0 |
| `bash scripts/sync-claims.sh && bash scripts/check-claims.sh` | 1 (same fixture failure) | 0 |
| `bash scripts/check-transcripts-reproduce.sh` | — | 0 |
| `bash scripts/no-network-test.sh` | 2 (no online baseline in outer sandbox) | 0 |
| `python3 scripts/check-tool-contracts.py` | 1 (debug sweep binary absent) | 0 |
| `bash stress/run.sh 60-cross-volume-exdev` | 2 (APFS image creation unavailable); shared-lock retries 3 | 0 × 10 |

Initial failures, copied from their command output:

```text
raw name fixture: Operation not permitted (os error 1)
test result: FAILED. 5 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.96s
error: test failed, to rerun pass `-p etude-core --test plan_snapshot`
FAIL cargo test --all failed; refusing to sync claims from an incomplete suite
FileNotFoundError: [Errno 2] No such file or directory: '/Users/festus/Documents/dev-projects/etudes-wt/cross-volume-window/target/debug/sweep'
AssertionError: sweep contract witness failed
no-network: host has no network access, so denial proves nothing
            run this on a machine that is online
    unproven cross-volume: EXDEV move is correct and interruption-safe (hdiutil create failed on this host)
```

The full stress suite was not run. No product code or sweep test hook changed.

## Evidence that still belongs to the supervisor

Read-only attempts to download all five cited CI log archives returned HTTP 403:
`Must have admin rights to Repository.` No `gh` command was run. This session
cannot measure historical CI copy times or determine from the quoted 32 MiB
line whether the 96/256 MiB attempts missed or subsequently proved the property.
The supervisor must inspect those complete logs and run the CI proof before
claiming CI reliability. Pushing and merging are reserved to the supervisor.
