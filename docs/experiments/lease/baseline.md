# Lease baseline: Process Compose and Pueue

Measured on 2026-09-05 using installed incumbents and generated temporary projects.
**Recommendation: revise the design direction around the missing ownership/cleanup receipt before advancing.**
Both incumbents handled independent cancellation; both left an escaped child live
while presenting terminal task status without residual evidence. No lease code,
protocol.md, lane tests, claims checker, or commits were produced. These are
capability judgments for the observed configurations, not the witness's acceptance
verdict. Phase 3 remains gated on a person reading this document.

## Scope, provenance, and how to read receipts

The six claims below are reproduced verbatim. “As shipped” means built-in behavior
with isolation configuration; “with modest configuration” means ordinary documented
options, probes, or scoped CLI timer recipes; “not at all” means the **full requested
property is absent in the evaluated interface/configuration**, not a proof that no
future plugin, patch, or arbitrary wrapper could implement it. Finite supportive
observations do not prove universal claims. Missing readiness is distinguished from
a false Ready report, and task-reference reuse from OS PID reuse.

The original generator was reused and executed, with extra synthetic project names
for escaped, resistant, and expiry workers. Every incumbent run generated its own private
`/tmp/lease-baseline-*` root. The review follow-up separately reads historical synthetic identities without starting tasks there. Existing binaries were used without installation or
service registration. The branch was already `seat/lease-baseline`, separate from
main; all changes remain uncommitted as requested. The original 120-second fixture
backstop was retained. No stress baseline was lowered or claimed measured.

Actual entry commands (the first Pueue attempt failed setup and is retained):

```sh
python3 docs/experiments/lease/synthetic/run.py process-compose
python3 docs/experiments/lease/synthetic/run.py pueue
# After creating the required private runtime directory, same command, fresh root:
python3 docs/experiments/lease/synthetic/run.py pueue
```

See [synthetic/README.md](synthetic/README.md) for the exact sequence and replay
requirements. Full timestamped argv/cwd/exit/stdout/stderr and endpoint observations
are in [Process Compose receipts](runs/process-compose/transcript.jsonl) and
[Pueue receipts](runs/pueue/transcript.jsonl). Numbered `n` references below identify
those original records. Command blocks reproduce exact commands; stdout is verbatim
unless explicitly labeled a JSON projection. Full stderr, including routine Process
Compose config-home diagnostics, remains in the raw receipts. HTTP observations are
actual `urllib.request.urlopen(url, timeout=1)` calls in the executed driver, not
invented curl runs. Identity files are hints; every critical observation also uses
the endpoint and targeted `ps -p PID -o pid=,ppid=,pgid=,stat=,lstart=,comm=`.

`attempt` precedes a command; `command` records its observed completion; `spawn`
needs its later `spawn-result` for exit/log evidence. Nonzero commands include
expected refusals. No self-reported zero counter establishes cleanliness.
`evidence.json` leaves independent `verified` false and author_verdict null;
`failed_commands` means nonzero commands, not an author-assigned unit verdict.

The local handoff is
`/Users/festus/Documents/Codex/2026-07-28/i/outputs/local-agent-capabilities-handoff.html`,
scoring v1.1.0, SHA-256
`cc992181c128882952242e9a6ab6f2d2383bcf84003eeb3a4ddb702505603060`.
[contracts.json](contracts.json) records hashes of the handoff, downloaded versioned
source archives, and inspected source files. [provenance.json](provenance.json)
records host/revision/CLI facts and the exact current-user quotations authorizing
incumbent runs while reserving cargo tests and the claims checker for the witness.
Those quotations are an author transcription, not an independently signed contract.
The earlier interpretation that this prohibited all execution was incorrect.

Sources read before selecting mutations:

- [Process Compose v1.122.0 process configuration](https://github.com/F1bonacc1/process-compose/blob/v1.122.0/src/types/process.go), [lifecycle](https://github.com/F1bonacc1/process-compose/blob/v1.122.0/src/app/process.go), and [schema](https://github.com/F1bonacc1/process-compose/blob/v1.122.0/schemas/process-compose-schema.json). Shutdown grace bounds cancellation; launch_timeout_seconds bounds daemon-output handling, not lease lifetime. No native durable lease deadline was identified.
- [Pueue v4.0.4 settings](https://github.com/Nukesor/pueue/blob/v4.0.4/pueue_lib/src/settings.rs), [task schema](https://github.com/Nukesor/pueue/blob/v4.0.4/pueue_lib/src/task.rs), [state restoration](https://github.com/Nukesor/pueue/blob/v4.0.4/pueue/src/daemon/internal_state/state.rs), and [kill handler](https://github.com/Nukesor/pueue/blob/v4.0.4/pueue/src/daemon/process_handler/kill.rs). Restore marks formerly running tasks Killed; it does not reconstruct child handles. Runtime/state/socket paths and state.json are explicit in these contracts.

No persistent PID field was guessed or forged. Where no ownership-recovery or native
expiry contract was found, that limitation is reported rather than invented.

## Capability matrix

| Claim (verbatim) | Process Compose | Pueue |
| --- | --- | --- |
| two tasks run concurrently and stopping one leaves the other and a synthetic unrelated listener unaffected | as shipped | with modest configuration |
| a stale record or PID reuse cannot authorize signaling a replacement process | not at all | not at all |
| started is never reported as ready until the configured condition is observed | with modest configuration | not at all |
| cancellation confirms which supported processes stopped and escaped descendants cannot be silently marked clean | not at all | not at all |
| supervisor interruption and explicit expiry exercise documented recovery | not at all | not at all |
| missing ownership evidence produces refusal, not guessed cleanup | with modest configuration | as shipped |

## Process Compose

Version: **v1.122.0 (commit 23b0aca)**. Exact configuration: [initial config](runs/process-compose/initial-config.json). Temporary root: `/tmp/lease-baseline-7tq9eq71`.

Readiness probe: [probe.py](runs/process-compose/probe.py). Timer configuration: [timer-config.json](runs/process-compose/timer-config.json). Recovery definitions: [recovery-config.json](runs/process-compose/recovery-config.json).

### 1. two tasks run concurrently and stopping one leaves the other and a synthetic unrelated listener unaffected

Classification: **as shipped** for the scope described below.

Task A (PID 40173) and task B (PID 40172) were simultaneously live. Stopping A removed its parent and ordinary child (PID 40175); both HTTP connections were refused and targeted ps returned exit 1 with empty output. Task B and the independently launched unrelated listener (PID 40131) still returned HTTP 200 with their original PIDs. The isolation settings and shutdown grace are explicit; concurrent startup and per-process stop are built-in behavior.

Supporting receipt numbers: 21, 23, 27, 31, 50, 51, 53, 54, 56, 57, 60, 64.

**Receipt 50, exit 0:**

```sh
process-compose -U -u /tmp/lease-baseline-7tq9eq71/pc.sock -L /tmp/lease-baseline-7tq9eq71/pc.log process stop task-a
```

stdout (verbatim):

```text
Successfully stopped: 'task-a'
```

**HTTP receipt 51:** `GET http://127.0.0.1:61176/` → None

```text
<urlopen error [Errno 61] Connection refused>
```

**HTTP receipt 57:** `GET http://127.0.0.1:61175/` → 200

```text
{"project": "task-b", "pid": 40172, "ready": true}
```

**HTTP receipt 60:** `GET http://127.0.0.1:61171/` → 200

```text
{"project": "unrelated", "pid": 40131, "ready": true}
```

### 2. a stale record or PID reuse cannot authorize signaling a replacement process

Classification: **not at all** for the scope described below.

A saved public reference, task-a, originally identified PID 40173. After that run stopped, process start task-a created PID 62799. Replaying the same stop reference returned success and the replacement endpoint disappeared; ps returned exit 1. The unrelated listener survived. This is a counterexample to stale public run references being generation-safe. It is not an OS PID-reuse observation, nor evidence that Process Compose signaled an independently launched unrelated PID. If the contract is narrowed to internal retained child handles only, that narrower PID-reuse property remains unproven.

Supporting receipt numbers: 121, 123, 126, 128, 129, 131, 132.

**Receipt 121, exit 0:**

```sh
process-compose -U -u /tmp/lease-baseline-7tq9eq71/pc.sock -L /tmp/lease-baseline-7tq9eq71/pc.log process start task-a
```

stdout (verbatim):

```text
Process task-a started
```

**Receipt 126 (saved-reference):**

```json
{
  "n": 126,
  "time": "2026-09-05T22:01:56.258491+00:00",
  "kind": "saved-reference",
  "reference": "task-a",
  "old_pid": 40173,
  "new_pid": 62799
}
```

**Receipt 128, exit 0:**

```sh
process-compose -U -u /tmp/lease-baseline-7tq9eq71/pc.sock -L /tmp/lease-baseline-7tq9eq71/pc.log process stop task-a
```

stdout (verbatim):

```text
Successfully stopped: 'task-a'
```

**HTTP receipt 129:** `GET http://127.0.0.1:61247/` → None

```text
<urlopen error [Errno 61] Connection refused>
```

### 3. started is never reported as ready until the configured condition is observed

Classification: **with modest configuration** for the scope described below.

The configured exec readiness_probe performs HTTP GET on the worker endpoint and succeeds only on HTTP 200. While the gate was closed, the incumbent reported Running / Not Ready / has_ready_probe=true and HTTP returned 503. After the gate opened, it reported Ready, with process_ready_time after the gate command, and HTTP returned 200. This supports the configured case; finite sampling does not prove the universal word never.

Supporting receipt numbers: 36, 37, 40, 41, 44, 45.

**Receipt 36, exit 0:**

```sh
process-compose -U -u /tmp/lease-baseline-7tq9eq71/pc.sock -L /tmp/lease-baseline-7tq9eq71/pc.log process get task-b -o json
```

Observed JSON projection of task state (full stdout in receipt):

```text
[
  {
    "name": "task-b",
    "status": "Running",
    "is_ready": "Not Ready",
    "has_ready_probe": true,
    "pid": 40172,
    "exit_code": 0,
    "is_running": true
  }
]
```

**HTTP receipt 37:** `GET http://127.0.0.1:61175/` → 503

```text
{"project": "task-b", "pid": 40172, "ready": false}
```

**Receipt 44, exit 0:**

```sh
process-compose -U -u /tmp/lease-baseline-7tq9eq71/pc.sock -L /tmp/lease-baseline-7tq9eq71/pc.log process get task-b -o json
```

Observed JSON projection of task state (full stdout in receipt):

```text
[
  {
    "name": "task-b",
    "status": "Running",
    "is_ready": "Ready",
    "has_ready_probe": true,
    "pid": 40172,
    "exit_code": 0,
    "is_running": true,
    "process_ready_time": "2026-09-05T15:01:39.149921-07:00"
  }
]
```

**HTTP receipt 45:** `GET http://127.0.0.1:61175/` → 200

```text
{"project": "task-b", "pid": 40172, "ready": true}
```

### 4. cancellation confirms which supported processes stopped and escaped descendants cannot be silently marked clean

Classification: **not at all** for the scope described below.

Ordinary parent and child both stopped in claim 1. The resistant worker stopped after the configured two-second SIGTERM grace and fallback; its state had exit_code=-1. But escaped parent PID 40367 was reported Completed with exit_code=0 after Successfully stopped, while child PID 40368 in its own process group/session continued returning HTTP 503. No residual/escaped classification appeared in the task receipt. Completed here is the parent task state, not an upstream promise to kill every descendant; it is insufficient for the requested lease receipt.

Supporting receipt numbers: 77, 79, 80, 82, 83, 85, 94, 96, 97, 99.

**Receipt 77, exit 0:**

```sh
process-compose -U -u /tmp/lease-baseline-7tq9eq71/pc.sock -L /tmp/lease-baseline-7tq9eq71/pc.log process stop escaped
```

stdout (verbatim):

```text
Successfully stopped: 'escaped'
```

**Receipt 79, exit 0:**

```sh
process-compose -U -u /tmp/lease-baseline-7tq9eq71/pc.sock -L /tmp/lease-baseline-7tq9eq71/pc.log process get escaped -o json
```

Observed JSON projection of task state (full stdout in receipt):

```text
[
  {
    "name": "escaped",
    "status": "Completed",
    "is_ready": "-",
    "has_ready_probe": false,
    "pid": 40367,
    "exit_code": 0,
    "is_running": false,
    "process_ready_time": "2026-09-05T15:01:41.973436-07:00"
  }
]
```

**HTTP receipt 83:** `GET http://127.0.0.1:61201/` → 503

```text
{"project": "child", "pid": 40368, "ready": false}
```

**Receipt 94, exit 0:**

```sh
process-compose -U -u /tmp/lease-baseline-7tq9eq71/pc.sock -L /tmp/lease-baseline-7tq9eq71/pc.log process stop resistant
```

stdout (verbatim):

```text
Successfully stopped: 'resistant'
```

**Receipt 96, exit 0:**

```sh
process-compose -U -u /tmp/lease-baseline-7tq9eq71/pc.sock -L /tmp/lease-baseline-7tq9eq71/pc.log process get resistant -o json
```

Observed JSON projection of task state (full stdout in receipt):

```text
[
  {
    "name": "resistant",
    "status": "Completed",
    "is_ready": "-",
    "has_ready_probe": false,
    "pid": 40400,
    "exit_code": -1,
    "is_running": false,
    "process_ready_time": "2026-09-05T15:01:44.488635-07:00"
  }
]
```

### 5. supervisor interruption and explicit expiry exercise documented recovery

Classification: **not at all** for the scope described below.

A separate synthetic Process Compose timer project ran sleep 4 followed by the scoped stop expiry CLI. Expiry PID 40654 stopped roughly four seconds after launch, well before the original 120-second worker backstop; task B and unrelated stayed live. Separately, SIGKILL of the directly held supervisor PID 40171 left task B answering HTTP 200. Restarting the same definitions with disabled=true prevented duplicate launches and yielded Disabled / pid=0 / is_running=false; stop refused and the old listener survived. The timer is modest shell/CLI configuration while supervisors are alive, not native durable lease expiry or restoration of prior ownership. No automatic reattachment/recovery guarantee was found in the inspected versioned contract. Durable expiry through supervisor loss is not satisfied by this recipe; crashing while the timer is pending was not measured.

Supporting receipt numbers: 107, 109, 110, 112, 113, 116, 136, 137, 138, 141, 143, 145, 146.

**Launch receipt 107:**

```sh
process-compose -f /tmp/lease-baseline-7tq9eq71/timer.json --disable-dotenv --no-server -L /tmp/lease-baseline-7tq9eq71/timer.log -t=false up
```

Direct child PID: 40666; cwd: `/tmp/lease-baseline-7tq9eq71`. Exit/logs are in the corresponding `spawn-result`.

**Receipt 109, exit 0:**

```sh
process-compose -U -u /tmp/lease-baseline-7tq9eq71/pc.sock -L /tmp/lease-baseline-7tq9eq71/pc.log process get expiry -o json
```

Observed JSON projection of task state (full stdout in receipt):

```text
[
  {
    "name": "expiry",
    "status": "Completed",
    "is_ready": "-",
    "has_ready_probe": false,
    "pid": 40654,
    "exit_code": 0,
    "is_running": false,
    "process_ready_time": "2026-09-05T15:01:48.907156-07:00"
  }
]
```

**Receipt 136 (signal-attempt):**

```json
{
  "n": 136,
  "time": "2026-09-05T22:01:57.525844+00:00",
  "kind": "signal-attempt",
  "pid": 40171,
  "signal": 9,
  "authority": "direct live Popen child"
}
```

**Receipt 137 (reaped):**

```json
{
  "n": 137,
  "time": "2026-09-05T22:01:57.529791+00:00",
  "kind": "reaped",
  "pid": 40171,
  "returncode": -9
}
```

**Receipt 143, exit 0:**

```sh
process-compose -U -u /tmp/lease-baseline-7tq9eq71/pc.sock -L /tmp/lease-baseline-7tq9eq71/pc.log process get task-b -o json
```

Observed JSON projection of task state (full stdout in receipt):

```text
[
  {
    "name": "task-b",
    "status": "Disabled",
    "is_ready": "-",
    "has_ready_probe": true,
    "pid": 0,
    "exit_code": 0,
    "is_running": false
  }
]
```

**Receipt 145, exit 1:**

```sh
process-compose -U -u /tmp/lease-baseline-7tq9eq71/pc.sock -L /tmp/lease-baseline-7tq9eq71/pc.log process stop task-b
```

stdout (verbatim):

```text
Failed to stop: 'task-b'
```

**HTTP receipt 146:** `GET http://127.0.0.1:61175/` → 200

```text
{"project": "task-b", "pid": 40172, "ready": true}
```

### 6. missing ownership evidence produces refusal, not guessed cleanup

Classification: **with modest configuration** for the scope described below.

After supervisor loss, the replacement supervisor had definitions but no live child handle for task B. With disabled=true to avoid accidental relaunch, stop task-b returned exit 1 and Failed to stop; an unknown synthetic name also returned exit 1. The orphan task B endpoint remained live. This is bounded refusal evidence with modest recovery configuration, not a proof of every possible missing-identity or race condition.

Supporting receipt numbers: 143, 154, 156, 157.

**Receipt 154, exit 1:**

```sh
process-compose -U -u /tmp/lease-baseline-7tq9eq71/pc.sock -L /tmp/lease-baseline-7tq9eq71/pc.log process stop task-b
```

stdout (verbatim):

```text
Failed to stop: 'task-b'
```

**Receipt 156, exit 1:**

```sh
process-compose -U -u /tmp/lease-baseline-7tq9eq71/pc.sock -L /tmp/lease-baseline-7tq9eq71/pc.log process stop unknown-synthetic-task
```

stdout (verbatim):

```text
Failed to stop: 'unknown-synthetic-task'
```

**HTTP receipt 157:** `GET http://127.0.0.1:61175/` → 200

```text
{"project": "task-b", "pid": 40172, "ready": true}
```


## Pueue

Version: **4.0.4 (client and daemon)**. Exact configuration: [initial config](runs/pueue/initial-config.json). Temporary root: `/tmp/lease-baseline-sdqt90ed`.

### 1. two tasks run concurrently and stopping one leaves the other and a synthetic unrelated listener unaffected

Classification: **with modest configuration** for the scope described below.

Configured parallel 4 (the default group starts with one slot). Task IDs 0 and 1, PIDs 93177 and 93178, were simultaneously live. kill 0 returned an attempted-action receipt; subsequent HTTP refusal plus empty ps output/exit 1 showed the ordinary parent and child stopped. Task B and the unrelated listener PID 93010 retained HTTP 200 with their original PIDs. Four slots also leave capacity for the later timer task; no stress threshold was reduced.

Supporting receipt numbers: 28, 30, 32, 34, 38, 42, 61, 62, 64, 65, 67, 68, 71, 75.

**Receipt 28, exit 0:**

```sh
pueue -c /tmp/lease-baseline-sdqt90ed/pueue.yml --color never parallel 4
```

stdout (verbatim):

```text
Parallel tasks setting for group "default" adjusted
```

**Receipt 61, exit 0:**

```sh
pueue -c /tmp/lease-baseline-sdqt90ed/pueue.yml --color never kill 0
```

stdout (verbatim):

```text
Tasks are being killed: 0
```

**HTTP receipt 62:** `GET http://127.0.0.1:61491/` → None

```text
<urlopen error [Errno 61] Connection refused>
```

**HTTP receipt 68:** `GET http://127.0.0.1:61490/` → 200

```text
{"project": "task-b", "pid": 93178, "ready": true}
```

**HTTP receipt 71:** `GET http://127.0.0.1:61485/` → 200

```text
{"project": "unrelated", "pid": 93010, "ready": true}
```

### 2. a stale record or PID reuse cannot authorize signaling a replacement process

Classification: **not at all** for the scope described below.

After the documented synthetic state.json was removed while the daemon was stopped, the restarted empty queue assigned replacement PID 2764 task ID 0. Replaying the saved old task ID 0 returned Tasks are being killed: 0; the replacement endpoint disappeared and ps returned exit 1. Unrelated stayed alive. The old reference therefore lacks a durable queue-generation identity across state loss. This is task-ID reuse, not forced OS PID reuse, and the replacement was a newly incumbent-managed task rather than an independently launched unrelated worker.

Supporting receipt numbers: 163, 165, 169, 170, 172, 173.

**Receipt 163, exit 0:**

```sh
pueue -c /tmp/lease-baseline-sdqt90ed/pueue.yml --color never add -p -w /tmp/lease-baseline-sdqt90ed/replacement 'exec /Library/Frameworks/Python.framework/Versions/3.14/bin/python3 /tmp/lease-baseline-sdqt90ed/worker.py --mode ordinary'
```

stdout (verbatim):

```text
0
```

**Receipt 169, exit 0:**

```sh
pueue -c /tmp/lease-baseline-sdqt90ed/pueue.yml --color never kill 0
```

stdout (verbatim):

```text
Tasks are being killed: 0
```

**HTTP receipt 170:** `GET http://127.0.0.1:61592/` → None

```text
<urlopen error [Errno 61] Connection refused>
```

### 3. started is never reported as ready until the configured condition is observed

Classification: **not at all** for the scope described below.

Task 1 was Running with identical start/enqueue timestamps both while HTTP returned 503 and after it returned 200. The inspected add options, TaskStatus, and settings offer no readiness condition or ready state. Thus there is no built-in configured readiness receipt to classify as ready after the condition. This is a missing capability, not a claim that Pueue printed a false Ready status. Adding a readiness/ownership wrapper would change the subject of the baseline.

Supporting receipt numbers: 30, 32, 47, 48, 51, 52, 55, 56.

**Receipt 47, exit 0:**

```sh
pueue -c /tmp/lease-baseline-sdqt90ed/pueue.yml --color never status --json
```

Observed JSON projection: tasks[*].status (full stdout in receipt):

```text
{
  "0": {
    "Running": {
      "enqueued_at": "2026-09-05T15:03:09.024494-07:00",
      "start": "2026-09-05T15:03:09.214071-07:00"
    }
  },
  "1": {
    "Running": {
      "enqueued_at": "2026-09-05T15:03:09.029690-07:00",
      "start": "2026-09-05T15:03:09.223174-07:00"
    }
  }
}
```

**HTTP receipt 48:** `GET http://127.0.0.1:61490/` → 503

```text
{"project": "task-b", "pid": 93178, "ready": false}
```

**Receipt 55, exit 0:**

```sh
pueue -c /tmp/lease-baseline-sdqt90ed/pueue.yml --color never status --json
```

Observed JSON projection: tasks[*].status (full stdout in receipt):

```text
{
  "0": {
    "Running": {
      "enqueued_at": "2026-09-05T15:03:09.024494-07:00",
      "start": "2026-09-05T15:03:09.214071-07:00"
    }
  },
  "1": {
    "Running": {
      "enqueued_at": "2026-09-05T15:03:09.029690-07:00",
      "start": "2026-09-05T15:03:09.223174-07:00"
    }
  }
}
```

**HTTP receipt 56:** `GET http://127.0.0.1:61490/` → 200

```text
{"project": "task-b", "pid": 93178, "ready": true}
```

### 4. cancellation confirms which supported processes stopped and escaped descendants cannot be silently marked clean

Classification: **not at all** for the scope described below.

The ordinary tree and SIGTERM-resistant worker stopped under normal kill (SIGKILL/process-group behavior). For escaped task 2, kill returned Tasks are being killed: 2 and status became Done/result=Killed while its child PID 94586 continued serving HTTP 503 in its own group/session. The receipt had no per-descendant residual/unproven report. Killed is Pueue task status, not a claimed whole-machine cleanup contract; the required lease receipt is missing.

Supporting receipt numbers: 88, 90, 91, 93, 94, 96, 105, 107, 108, 110.

**Receipt 88, exit 0:**

```sh
pueue -c /tmp/lease-baseline-sdqt90ed/pueue.yml --color never kill 2
```

stdout (verbatim):

```text
Tasks are being killed: 2
```

**Receipt 90, exit 0:**

```sh
pueue -c /tmp/lease-baseline-sdqt90ed/pueue.yml --color never status --json
```

Observed JSON projection: tasks[*].status (full stdout in receipt):

```text
{
  "0": {
    "Done": {
      "enqueued_at": "2026-09-05T15:03:09.024494-07:00",
      "start": "2026-09-05T15:03:09.214071-07:00",
      "end": "2026-09-05T15:03:13.752214-07:00",
      "result": "Killed"
    }
  },
  "1": {
    "Running": {
      "enqueued_at": "2026-09-05T15:03:09.029690-07:00",
      "start": "2026-09-05T15:03:09.223174-07:00"
    }
  },
  "2": {
    "Done": {
      "enqueued_at": "2026-09-05T15:03:14.642704-07:00",
      "start": "2026-09-05T15:03:14.658375-07:00",
      "end": "2026-09-05T15:03:14.961306-07:00",
      "result": "Killed"
    }
  }
}
```

**HTTP receipt 94:** `GET http://127.0.0.1:61535/` → 503

```text
{"project": "child", "pid": 94586, "ready": false}
```

**Receipt 105, exit 0:**

```sh
pueue -c /tmp/lease-baseline-sdqt90ed/pueue.yml --color never kill 3
```

stdout (verbatim):

```text
Tasks are being killed: 3
```

### 5. supervisor interruption and explicit expiry exercise documented recovery

Classification: **not at all** for the scope described below.

A delayed task scheduled with --delay 4 invoked the scoped kill 4 command. Task 4 became Killed around 4.5 seconds after launch; timer task 5 became Success; ps/HTTP observations confirmed expiry before the worker backstop and survival of task B and unrelated. Separately, SIGKILL of the daemon left task B live. Restart with the same state changed task 1 from Running to Done/result=Killed although its endpoint still returned HTTP 200. This matches restore_state in the pinned source, which changes status without reattaching children. The timer recipe requires queue capacity and a live daemon; it is not native durable lease expiry. A pending-timer crash was not measured. The observed recovery does not provide truthful stopped/residual evidence.

Supporting receipt numbers: 119, 121, 122, 124, 125, 128, 132, 133, 134, 137, 139, 141, 142.

**Receipt 119, exit 0:**

```sh
pueue -c /tmp/lease-baseline-sdqt90ed/pueue.yml --color never add --delay 4 -w /tmp/lease-baseline-sdqt90ed 'pueue -c /tmp/lease-baseline-sdqt90ed/pueue.yml --color never kill 4'
```

stdout (verbatim):

```text
New task added (id 5). It will be enqueued at 15:03:23
```

**Receipt 121, exit 0:**

```sh
pueue -c /tmp/lease-baseline-sdqt90ed/pueue.yml --color never status --json
```

Observed JSON projection: tasks[*].status (full stdout in receipt):

```text
{
  "0": {
    "Done": {
      "enqueued_at": "2026-09-05T15:03:09.024494-07:00",
      "start": "2026-09-05T15:03:09.214071-07:00",
      "end": "2026-09-05T15:03:13.752214-07:00",
      "result": "Killed"
    }
  },
  "1": {
    "Running": {
      "enqueued_at": "2026-09-05T15:03:09.029690-07:00",
      "start": "2026-09-05T15:03:09.223174-07:00"
    }
  },
  "2": {
    "Done": {
      "enqueued_at": "2026-09-05T15:03:14.642704-07:00",
      "start": "2026-09-05T15:03:14.658375-07:00",
      "end": "2026-09-05T15:03:14.961306-07:00",
      "result": "Killed"
    }
  },
  "3": {
    "Done": {
      "enqueued_at": "2026-09-05T15:03:16.823623-07:00",
      "start": "2026-09-05T15:03:17.077787-07:00",
      "end": "2026-09-05T15:03:17.380631-07:00",
      "result": "Killed"
    }
  },
  "4": {
    "Done": {
      "enqueued_at": "2026-09-05T15:03:19.212403-07:00",
      "start": "2026-09-05T15:03:19.495855-07:00",
      "end": "2026-09-05T15:03:24.032706-07:00",
      "result": "Killed"
    }
  },
  "5": {
    "Done": {
      "enqueued_at": "2026-09-05T15:03:23.725137-07:00",
      "start": "2026-09-05T15:03:23.730548-07:00",
      "end": "2026-09-05T15:03:24.032711-07:00",
      "result": "Success"
    }
  }
}
```

**Receipt 132 (signal-attempt):**

```json
{
  "n": 132,
  "time": "2026-09-05T22:03:26.718957+00:00",
  "kind": "signal-attempt",
  "pid": 93030,
  "signal": 9,
  "authority": "direct live Popen child"
}
```

**Receipt 133 (reaped):**

```json
{
  "n": 133,
  "time": "2026-09-05T22:03:26.720320+00:00",
  "kind": "reaped",
  "pid": 93030,
  "returncode": -9
}
```

**Receipt 139, exit 0:**

```sh
pueue -c /tmp/lease-baseline-sdqt90ed/pueue.yml --color never status --json
```

Observed JSON projection: tasks[*].status (full stdout in receipt):

```text
{
  "0": {
    "Done": {
      "enqueued_at": "2026-09-05T15:03:09.024494-07:00",
      "start": "2026-09-05T15:03:09.214071-07:00",
      "end": "2026-09-05T15:03:13.752214-07:00",
      "result": "Killed"
    }
  },
  "1": {
    "Done": {
      "enqueued_at": "2026-09-05T15:03:09.029690-07:00",
      "start": "2026-09-05T15:03:09.223174-07:00",
      "end": "2026-09-05T15:03:26.729553-07:00",
      "result": "Killed"
    }
  },
  "2": {
    "Done": {
      "enqueued_at": "2026-09-05T15:03:14.642704-07:00",
      "start": "2026-09-05T15:03:14.658375-07:00",
      "end": "2026-09-05T15:03:14.961306-07:00",
      "result": "Killed"
    }
  },
  "3": {
    "Done": {
      "enqueued_at": "2026-09-05T15:03:16.823623-07:00",
      "start": "2026-09-05T15:03:17.077787-07:00",
      "end": "2026-09-05T15:03:17.380631-07:00",
      "result": "Killed"
    }
  },
  "4": {
    "Done": {
      "enqueued_at": "2026-09-05T15:03:19.212403-07:00",
      "start": "2026-09-05T15:03:19.495855-07:00",
      "end": "2026-09-05T15:03:24.032706-07:00",
      "result": "Killed"
    }
  },
  "5": {
    "Done": {
      "enqueued_at": "2026-09-05T15:03:23.725137-07:00",
      "start": "2026-09-05T15:03:23.730548-07:00",
      "end": "2026-09-05T15:03:24.032711-07:00",
      "result": "Success"
    }
  }
}
```

**Receipt 141, exit 1:**

```sh
pueue -c /tmp/lease-baseline-sdqt90ed/pueue.yml --color never kill 1
```

stdout (verbatim):

```text
(empty)
```

stderr (verbatim):

```text
The command failed for tasks: 1
```

**HTTP receipt 142:** `GET http://127.0.0.1:61490/` → 200

```text
{"project": "task-b", "pid": 93178, "ready": true}
```

### 6. missing ownership evidence produces refusal, not guessed cleanup

Classification: **as shipped** for the scope described below.

With the daemon stopped, only its documented private state.json was renamed to state.json.saved. Restart produced an empty tasks map. kill 1 returned exit 1, empty stdout, and The command failed for tasks: 1 on stderr. The orphan task B listener remained alive. A first restart with stale Running state also refused kill 1 after classifying it Killed. This is refusal for absent/non-running task ownership in the measured case; it does not make reused task IDs safe (claim 2).

Supporting receipt numbers: 150, 151, 152, 153, 155, 157, 158.

**Receipt 152, exit 0:**

```sh
/Library/Frameworks/Python.framework/Versions/3.14/bin/python3 -c 'from pathlib import Path; Path('"'"'/tmp/lease-baseline-sdqt90ed/state/state.json'"'"').rename('"'"'/tmp/lease-baseline-sdqt90ed/state/state.json.saved'"'"')'
```

stdout (verbatim):

```text
(empty)
```

**Receipt 155, exit 0:**

```sh
pueue -c /tmp/lease-baseline-sdqt90ed/pueue.yml --color never status --json
```

Observed JSON projection: tasks[*].status (full stdout in receipt):

```text
{}
```

**Receipt 157, exit 1:**

```sh
pueue -c /tmp/lease-baseline-sdqt90ed/pueue.yml --color never kill 1
```

stdout (verbatim):

```text
(empty)
```

stderr (verbatim):

```text
The command failed for tasks: 1
```

**HTTP receipt 158:** `GET http://127.0.0.1:61490/` → 200

```text
{"project": "task-b", "pid": 93178, "ready": true}
```

## Setup failure, cleanup, and remaining limits

The first Pueue attempt is preserved under
[runs/pueue-setup-failure](runs/pueue-setup-failure/transcript.jsonl), with its
[daemon error](runs/pueue-setup-failure/supervisor.log) and [driver output](runs/pueue-setup-failure/driver.log).
Pueued could not create `runtime/pueue.pid` because the private runtime directory
had not been created. Client commands exited 1 and no managed task launched. The
recipe now creates that directory. This is a recorded setup failure, not a
capability counterexample. The retry used a new generated root, retained the same
worker lifetimes and scenarios, and did not weaken any refusal.

The normal experiment cleanup shuts down only the explicitly configured daemon
and signals only the directly held unrelated-worker handle. Orphaned and escaped
workers are allowed to reach their original bounded lifetime, with final endpoint
and targeted process observations in each transcript. That wait is containment,
not expiry evidence or incumbent cleanup credit. Successful command exits alone
are not used to claim those residuals stopped. Both completed drivers exited 0.
Final recorded HTTP probes were refused and every targeted ps lookup returned
exit 1 with empty stdout: Process Compose receipts 176–203 and Pueue receipts
192–219. These observations occurred after the original fixture backstops,
not as part of incumbent cancellation. The setup-failure unrelated listener
was also observed after its backstop. Its first final probe was sandbox-denied,
which was not treated as exit evidence; replay with authorized access returned
connection refused and ps exit 1 with empty stdout. See
[final-observation.json](runs/pueue-setup-failure/final-observation.json).

OS PID reuse, cancellation races, all possible descendants, cross-platform behavior,
and expiry pending across an abrupt supervisor crash remain unproven. No external
claims checker or stress suite was run. In particular, a stopped parent, absent
listener, or false is_running field does not by itself establish complete cleanup.
The timer recipes use ordinary commands but do not supply durable run identity,
reattachment, or truthful residual receipts. Building such an adapter would be
phase 3, which this unit does not authorize.

## Trace and review boundary

`emit` was available and accepted decisions as they were made. Its actual reply
says decisions are spooled and enter the trace when the seat closes. Its `--help`
exposes no close or trace-hash operation. A closed trace hash is therefore still
unavailable to this author; `trace_hash` remains null, explicitly pending seat
closure. [decision-spool.jsonl](decision-spool.jsonl) is a snapshot of emitted
records, **not a closed trace**. [artifact-sha256.json](artifact-sha256.json)
identifies the current artifact bytes, including actual runtime receipts; hashes
do not make an uncommitted diff immutable or prove execution independently.
The final digest is emitted into the open seat record so the later witness can
bind the diff after closure. No digest is invented and no self-issued pass is
substituted for that check.

The repository ignores `docs/`; only this unit's artifact files are marked
intent-to-add so they appear in the review diff, without staging their contents.
The operator must decide whether to commit these scripts and receipts. The user
explicitly asked for uncommitted work, so the earlier requirement for a committed
fixture is intentionally deferred to that review. The prior “no execution” blocker
has been removed; both incumbents actually ran. The closed trace and independent
verdict remain outside this author's reach.


## Review repairs: bounded follow-ups, 2026-09-05

The original run receipts and original 35-line decision spool are preserved.
These additions address evidence gaps; they do not issue the lane's verdict.
Only the disputed configuration surfaces and escaped-child observations were run
again. No acceptance checker, cargo tests, protocol, or lease implementation ran
or was written. The fixture lifetime is still 120 seconds.

### Provenance and artifact integrity

Before editing any original artifact, recomputation of **all 40** listed hashes
found 40 matches. The per-file expected and observed values are retained in
[original-manifest-recomputation.json](runs/review-followup/original-manifest-recomputation.json).
This is artifact byte accounting, not independent verification of the experiment.
The old provenance is retained at
[provenance-before-review.json](runs/review-followup/provenance-before-review.json).
The refreshed [provenance.json](provenance.json) inventories every unit artifact,
including the manifest, both Pueue run directories, and decision snapshots, and
records Git status after the additions exist. Git's status format can collapse
untracked directories; the separate sorted file inventory supplies file coverage.

A SHA-256 manifest cannot contain the SHA-256 of its own final bytes without a
circular dependency. The manifest explicitly excludes itself and its digest is
emitted into the seat record after all covered files are finalized. The digest
therefore binds the manifest *through that external record*, not through a
self-asserted checksum inside the diff. There is no commit, signature, or closed
trace available here. Authentication and detection of replacement of both files
and manifest remain conditional on the later witness retaining that external
record. The operator's instruction to leave work uncommitted is preserved.

### Historical identity, cleanup, and timing

The original Process Compose transcript spans **22:01:35.599038–22:03:51.182379
UTC**, 208 records. Its active scenario sequence ended near 22:02:00; containment
observations continued. Original n 194 records `ps` exit 1 with empty stdout for
40368 at 2026-09-05T22:03:51.172353+00:00; n 192
records connection refusal. These are historical final observations, not proof
of the exact instant or cause of exit. Original n 85 uses `comm`, so it cannot
independently supply the child's historical argv. Worker identity, parent
`child_pid`, and endpoint response corroborate the fixture attribution, but the
missing argv cannot be recovered retroactively.

`python3 docs/experiments/lease/synthetic/observe_previous.py` made fresh read-only
observations, retained in
[historical-residuals.jsonl](runs/review-followup/historical-residuals.jsonl).
All 19 observed historical PIDs returned `ps` exit 1 and empty stdout; all their
recorded endpoints refused connections. No historical PID was signaled. Current
absence does not establish continuous absence or repair historical identity proof.

[timeline.json](runs/review-followup/timeline.json) explicitly maps 20 runtime
spool decisions to their affected original receipt numbers, including the failed
Pueue setup. Each mapped decision timestamp precedes its action. Readiness maps
to the gate mutation, not the preceding diagnostic that prompted the decision.
The other 15 spool lines concern preparation, documentation, or artifact binding;
they have no incumbent action counterpart and are listed separately. This is a
comparison of mutable local timestamps, not authenticated proof that all 35 lines
were emitted when claimed. [review-decision-spool.jsonl](review-decision-spool.jsonl) snapshots the repair
spool through the binding decision, before the final digest emission; the digest
record must be read from the external seat record. The new driver brackets each `emit` invocation with
`decision-attempt` and `decision-result` before its dependent action.

### Process Compose dependency conditions

Exact entry command:

```sh
python3 docs/experiments/lease/synthetic/followup.py process-compose
```

Installed version remains **v1.122.0 / 23b0aca**. The generated project and full
commands are in [manifest](runs/followup-process-compose/manifest.json),
[config](runs/followup-process-compose/config.json), and
[transcript](runs/followup-process-compose/transcript.jsonl).
The configuration uses `process_started`, `process_healthy`, `process_completed`,
and `process_completed_successfully` dependencies on one escaped parent, with
an HTTP exec readiness probe. The documented condition definitions were read in
[versioned Process Compose source](https://github.com/F1bonacc1/process-compose/blob/v1.122.0/src/types/process.go).

At gate-closed n 19, only `process_started.receipt` existed. At gate-open n 29,
`process_healthy.receipt` also existed. After cancellation n 39, both completion
markers existed, including `process_completed_successfully.receipt`. These marker
contents are UTC timestamps written by the exact Python commands in the saved
configuration. The child's full argv and parent relationship were captured before
cancellation (n 14), then its reparented full argv and live endpoint after it
(n 44–45). Thus completion dependencies add useful scheduling semantics, but
this evaluated configuration still does not supply a descendant cleanup receipt.

Receipt 36, exit 0:

```sh
process-compose -U -u /tmp/lease-baseline-yjwzkwu5/pc.sock -L /tmp/lease-baseline-yjwzkwu5/pc.log process stop source
```

stdout, verbatim:

```text
Successfully stopped: 'source'
```

Receipt 44, exit 0:

```sh
/bin/ps -ww -p 80019 -o pid=,ppid=,pgid=,stat=,lstart=,args=
```

stdout, verbatim:

```text
80019     1 80019 Ss   Sat Sep  5 15:15:57 2026     /Library/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python /private/tmp/lease-baseline-yjwzkwu5/worker.py --lifetime 120
```

Receipt 45: `GET http://127.0.0.1:62801/` → 503:

```text
{"project": "child", "pid": 80019, "ready": false}
```

### Pueue completion callback

Exact entry command:

```sh
python3 docs/experiments/lease/synthetic/followup.py pueue
```

Installed version remains **4.0.4**. See the generated
[manifest](runs/followup-pueue/manifest.json),
[config](runs/followup-pueue/config.json),
[callback script](runs/followup-pueue/callback.py), and
[transcript](runs/followup-pueue/transcript.jsonl).
The documented `daemon.callback` runs the saved Python script with `{{ id }}` and
`{{ result }}` parameters. The callback is a completion hook in the
[versioned Pueue source](https://github.com/Nukesor/pueue/blob/v4.0.4/pueue/src/daemon/callbacks.rs).
It was absent at both gate snapshots (n 21, 31), while the task was Running.
After cancellation, n 41 observed this exact callback output:

```json
{"time": "2026-09-05T22:16:10.259441+00:00", "id": "0", "result": "Killed"}
```

The callback successfully exposed parent termination. At that point the escaped
child still answered HTTP. It neither added a native readiness state nor supplied
a descendant inventory in this configuration. A callback that performs additional
ownership tracking would require separately evaluated code; this run does not
rule out such a contribution or integration.

Receipt 38, exit 0:

```sh
pueue -c /tmp/lease-baseline-ruvl5kyf/pueue.json --color never kill 0
```

stdout, verbatim:

```text
Tasks are being killed: 0
```

Receipt 46, exit 0:

```sh
/bin/ps -ww -p 80086 -o pid=,ppid=,pgid=,stat=,lstart=,args=
```

stdout, verbatim:

```text
80086     1 80086 Ss   Sat Sep  5 15:16:04 2026     /Library/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python /private/tmp/lease-baseline-ruvl5kyf/worker.py --lifetime 120
```

Receipt 47: `GET http://127.0.0.1:62860/` → 503:

```text
{"project": "child", "pid": 80086, "ready": false}
```

### Follow-up residual containment and scope of negative cells

Both supervisors exited with code 0. Process Compose follow-up n 110–116 and
Pueue follow-up n 117–123 are explicit final residual observations: parent and
child `ps` exit 1 with empty stdout, paired with connection refusal. The complete
commands and observed output are in the linked transcripts. No recorded PID
hint authorized cleanup signals. The escaped children survived incumbent
cancellation, were observed repeatedly with full argv, then were observed absent
after the unchanged fixture backstop. This is observed fixture containment,
**not verified incumbent cleanup**, and does not establish a precise exit time.

The matrix's negative cells apply only to the evaluated interfaces and recipes.
The four distinct disputed properties are bounded as follows:

- **Stale record/PID reuse:** the old name/task-ID replay observations remain
  counterexamples for generation-free public task references. Dependency states
  and completion callbacks do not change the stop/kill command operands used here.
  Actual OS PID reuse and internal child-handle safety remain unproven; this is
  not a universal negative about either tool's signal implementation.
- **Readiness:** Process Compose's probe plus healthy dependency supports the
  configured readiness case. Pueue's completion callback was exercised and did
  not create a readiness state while the task was running. A custom readiness
  adapter remains unevaluated; no false native Ready field is alleged.
- **Cancellation receipts:** completion dependencies and callbacks were actually
  exercised while full-argv-identified escaped children remained live. Parent
  terminal status is not a clean-tree assertion by either tool; the missing
  property is an explicit supported-process/residual receipt for the lease caller.
- **Interruption/expiry recovery:** preserve the original crash and timer runs.
  These new hooks do not measure expiry pending across a crash, and that case
  remains unproven. No claim of impossibility for arbitrary durable wrappers is
  made. Designing one is outside phase 2.

These limits withdraw any reading of “not at all” as exhaustive impossibility.
The concrete missing property motivating revision is still the observed lack of
an ownership-aware cancellation/recovery receipt in the configurations evaluated.
A person must read this baseline before phase 3; there is no author-issued pass.

revise — the concrete missing property is a cancellation/recovery receipt that retains launch identity and explicitly reports escaped or orphaned descendants as residual/unproven. The evaluated configurations did not provide it; alternative adapters and OS PID-reuse safety remain unproven, and phase 3 remains gated on a person reading this baseline.
