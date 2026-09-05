# Synthetic incumbent experiment recipe

`create.py` creates a fresh private `lease-baseline-*` temporary directory and
prints its manifest. It takes no existing project as input. `run.py` actually
executes that generator, starts the selected installed incumbent, drives its
CLI, and records observations. These are phase-2 incumbent experiments, not
the lane's acceptance checker, cargo tests, or a lease implementation.

Run each once from this checkout (the evidence directory must not already exist):

```sh
python3 docs/experiments/lease/synthetic/run.py process-compose
python3 docs/experiments/lease/synthetic/run.py pueue
```

The script requires `emit` and stops if a decision cannot be recorded. Declare
the run part through `emit --id ... --needs ...` before invoking it. It requires
permission to start local synthetic processes and listen on loopback/temporary
Unix sockets. `run.py` is deliberately a recording recipe without acceptance
assertions or pass/fail verdicts. Its HTTP readiness probe exits successfully
only for HTTP 200; that is incumbent configuration, not an acceptance test.

The script saves `runs/<tool>/transcript.jsonl`, the manifest, worker bytes,
configuration snapshots, and supervisor logs. Every receipt has a timestamp
and sequence number. Command receipts include exact argv, shell rendering,
cwd, stdout, stderr, and exit status. `attempt` records precede execution;
`spawn` records require the later `spawn-result` for exit status and logs.
`http` records come from Python urllib against the exact loopback URL shown.
`identity-hint` is the worker's self-report, followed by HTTP and targeted `ps`
observations. An HTTP 503 is evidence of a live listener, not readiness.
Connection refusal alone cannot prove a process exited; `ps` supplies a
separate observation, including zombies if present.

All worker modes retain the original 120-second lifetime (maximum selectable
600 seconds). This is a containment backstop, **not incumbent expiry** or a
stress threshold. The worker listens on an ephemeral loopback port, returns
503 until `allow-ready` is created, and then returns 200. Ordinary `tree`
children share the parent's process group. `escaped` children create a new
session. `resistant` ignores SIGTERM. Each child has its own endpoint and
120-second backstop. The parent deliberately does not clean up children.

State, config, sockets, project directories, logs, and fixture gates are under
the generated temp root. The child environment is a recorded minimal map;
no full host environment is copied into Pueue receipts. Process Compose uses
an explicit config and socket and disables dotenv. Pueue uses an explicit
config with dedicated state, runtime, aliases, and socket paths. No default
daemon is contacted. No installation or service registration is performed.

The scripted sequence observes closed/open readiness, concurrent tasks,
targeted cancellation of a tree, escaped and resistant workers, explicit
four-second timer cancellation, stale task-reference reuse, abrupt supervisor
loss/restart, and missing state. Process Compose's timer is a second synthetic
Process Compose project invoking the first project's scoped stop CLI. Pueue's
timer is a delayed task invoking its scoped kill CLI. These recipes are
**not native durable lease expiry**: they need a live supervisor, queue
capacity, and a still-valid task reference. They do not prove expiry recovery
across a crash. The worker backstop is never substituted for timer evidence.

For Process Compose recovery, the driver preserves process definitions but
marks them disabled before restarting, to observe lost ownership without
launching duplicates. For Pueue it first restarts the same state, then stops
the daemon, renames its documented `state.json`, and restarts empty state.
It never fabricates internal PID fields. Stale process-name/task-id replay is
observed separately from actual OS PID reuse; OS PID reuse remains unproven.

Only direct live `Popen` children are signaled by the driver, using unreaped
handles. Incumbent CLIs signal their own synthetic tasks. No PID from a fixture
record authorizes driver cleanup, and no name/port-wide host cleanup occurs.
Residual descendants are allowed to reach their original lifetime; final HTTP
and targeted process observations are retained. A driver error leaves bounded
workers to expire; inspect the retained transcript and isolated supervisor
handle before any further cleanup. Do not use default/global daemon commands.

The user requested uncommitted work. These scripts are supplied in the diff
for the operator to review and commit; the author does not commit them. No
stress baseline was changed or claimed measured.


Review follow-ups (fresh output directories required):

```sh
python3 docs/experiments/lease/synthetic/followup.py process-compose
python3 docs/experiments/lease/synthetic/followup.py pueue
python3 docs/experiments/lease/synthetic/observe_previous.py
```

Declare the parts with emit before entry. `followup.py` reuses `create.py`, keeps
its 120-second workers unchanged, exercises documented dependency/callback
configuration, and records full argv, HTTP, markers, supervisor exit and final
residual observations. It never uses saved PIDs to authorize signals. The
read-only `observe_previous.py` observes historical synthetic identity hints;
it does not create a new project or start any tasks. Fresh absence cannot recover
historical argv. These are observation recipes, not the lane's witnesses.
