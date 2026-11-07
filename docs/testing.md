# Testing and checking the claims

Back to the [README](../README.md).

## Check the claims in a minute

Every étude ships the same two witnesses. Neither is a promise; both are
commands you can run.

```sh
cargo test --all                # 778 tests
scripts/no-network-test.sh      # all but the process-control test under socket denial
```

The second one proves the sandbox works *before* running the suite: a control
program that opens a TCP connection must succeed unsandboxed and be denied
under the profile. On macOS, all tests except the SIGTERM process-control
witness run under denial; Seatbelt hides child enumeration from that test, so
the script runs it separately outside denial. That witness makes no network
calls. The denied suite witnesses that its exercised code paths open no
sockets; it does not exercise the `security`, `unzip`, `gunzip`, or `tar`
subprocess call sites.

It uses `sandbox-exec`, so it is macOS like everything else here. Run it
anywhere else and it exits `2` saying the claim cannot be made on this host,
rather than passing. A witness that quietly passes where it cannot observe
anything is worse than no witness.

Beyond that, `etude-core` has **zero dependencies**, asserted by a test. There is no third-party code in the path that decides what happens
to your files.

## Try it without risking a real folder

The fixture generator builds a deliberately adversarial tree: tax forms,
medical records, an identity document, a filename containing a tab, a 200-character
filename, symlinks that point outside the directory. No real file of yours is
read during development or testing.

```sh
cargo run -p fixtures --bin mkfx -- /tmp/demo
cargo run -p sweep-cli --bin sweep -- /tmp/demo
cargo run -p stash-cli --bin stash -- /tmp/demo --for 3d
```

## What is broken

I wrote an adversarial harness and pointed it at my own tools: 80 scenarios
covering macOS filesystem hazards, crashes mid-apply, races between plan and
apply, exact item-cap boundaries, and real disk images for full, read-only and
case-sensitive volumes.

```sh
bash stress/run.sh        # 80 scenarios
```

Every failed assertion is actionable. Timing is recorded as a measurement, but
it cannot exempt an integrity assertion in the same scenario.

The best story in the tracker is closed: an earlier fix swapped `rename` for
`link` plus `unlink` to stop silent overwrites, and that opened a crash window
where a killed process left one file under two names. It is now a single
atomic rename, and the recovery for old journals knows which link was sweep's
by its position in the journal rather than by guessing from inodes.

There is also an `unproven` count, kept separate from the passes on purpose. A
hazard that could not be exercised on this machine is not a hazard that passed.

Each run writes a compact JSON bundle under `stress/results/` (or
`STRESS_RESULTS_DIR`): one row per case with its contract, capability, tier,
duration, verdict, child exit, and any retained failure evidence. CI uploads
that bundle with failure-only transcripts and assertion records.

A scenario can also run directly against built release binaries:

```sh
BIN="$PWD/target/release" SCENARIO=85-scenario-exit-status bash stress/scenarios/85-scenario-exit-status.sh
bash scripts/check-scenario-outcomes.sh  # compare direct runs, the batch, and the origin/main runner
```

The harness records assertions separately from stdout. Failed assertions produce
exit 1 even after `exit 0`, cleanup, command substitution, or `exec`. Subshells
still have their own shell counters; their assertion records reach the wrapper.
Runs with only unproven assertions exit 2; a run with no assertions fails.

## Layout

```
crates/
  etude-core/    scan, plan, apply, journal-first undo, zero dependencies
  etude-keep/    journal encryption (XChaCha20-Poly1305, keychain or supplied key)
  etude-read/    content inspection, mlock'd, zeroed, never persisted
  etude-cli-support/ the progress layer, result envelope, refusal codes and contracts shared by the CLIs
  sweep-cli/     bin: sweep
  stash-cli/     bin: stash
  unpack-cli/    bin: unpack, dispatches to system tools, parses nothing
  fixtures/      synthetic adversarial trees; no real file is read in testing
```
