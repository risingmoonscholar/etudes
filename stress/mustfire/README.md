# What makes a stress scenario a good test case

A stress scenario is a good test case when all 5 rules are true:

1. The scenario tests a promise that the docs or the catalog contract make.
2. The scenario checks every part of its catalog contract, and it claims nothing that it does not check.
3. A real bug can make the scenario fail. A bug must give FAIL, not "unproven" and not a pass.
4. The scenario gives the same result on every run. To inject a fault while a tool runs, stop the tool, confirm that the tool is stopped, then inject the fault. Do not sleep and hope.
5. The scenario is safe. It uses the harness's random journal key, a throwaway state folder and a throwaway home folder. It never touches the login keychain. It never drives Finder or any other desktop app.

## The 3 checks

| Check | What it proves | Command |
|---|---|---|
| Planted bugs | Rule 3. Each scenario catches the bugs listed for it in `cases.json`. | `BIN=target/release python3 stress/mustfire/run.py` |
| Source rules | Rules 4 and 5, and the parts of rule 3 that the source shows. | `python3 scripts/check-stress-taste.py` |
| Suite rules | Floors, pinned unproven counts, the early-exit message, and coverage of every audited scenario. | `python3 scripts/test_stress_rules.py` |

All 3 checks fail on the suite as it was audited for 0.5.5. The work is done when all 3 pass, and the full stress run passes:

1. `BIN=target/release python3 stress/mustfire/run.py` prints `ok mustfire: 0 planted bug(s) missed`.
2. `python3 scripts/check-stress-taste.py` prints `ok stress taste`.
3. `python3 scripts/test_stress_rules.py` passes every test.
4. `stress/run.sh` passes. Every scenario meets its floor, and its unproven count equals its pin in `stress/unproven.json`.
5. CI runs checks 2 and 3, and the stress job compares unproven counts with the pins.

## How a planted bug works

`run.py` makes a scratch BIN folder. In that folder, `sweep`, `stash` and `unpack` are shims that run `mutant.py`. A call that the planted bug does not target runs the real binary unchanged. A targeted call runs the real binary, then does one wrong thing: it loses a file, changes bytes, hides a line, ignores a signal, or reports the wrong exit or reason code.

Each case is caught only when its scenario reports FAIL. A pass, an unproven result or a timeout is a miss.

## Rules for the fixes

- Change the scenario, not the planted bug, the source rules or the suite tests. If you think a planted bug or rule is wrong, say so in the PR. Do not change it.
- Never lower a floor. Never remove or weaken an assertion. Raise floors from a measured full run.
- A scenario that cannot set up its condition on this host reports unproven. A scenario that sets up its condition and sees the bug reports FAIL.
- Pin each scenario's unproven count in `stress/unproven.json`. List only scenarios with unproven results.
- If a stronger scenario finds a real product bug, stop. Report the bug with the scenario output. Do not change product code in the same PR.
- Run scenarios only from outside the repository, with `SCENARIO` set.
- Never run a sweep or stash build older than 0.5.3. Never run `/usr/bin/security`.

## Gaps that the checking seat confirms by reading

No planted bug or source rule shows these gaps. The seat that checks the PR confirms each one by reading the scenario.

| Scenario | The fix to confirm |
|---|---|
| 15-real-project-layouts | Each document layout puts 3 or more referenceable files at scan level, and LooseDoc has 3 or more `.wav` files, so the hold comes from the reference rule and not from the 3-file floor. The check at line 466 also asserts the exit code or the `Documents` group. |
| 60-rlimit-nofile | Apply under the low limit asserts the journal exists and undo exits 0 with all 1000 files home before any manual reset. A refusal asserts no panic. (A planted bug cannot run under the low limit.) |
| 106-failure-steps-are-true | If `forget` removes the journal from the unreadable folder, the other outcome is asserted in full (exit 0, "Removed 1 journal"), or the folder is locked with `chflags uchg` so the case is always proven. |
| 109-unpack-messages | `RACED` is set only when `mkdir` succeeds. Then exit 2 and an empty folder are required. A missing `xattr` reports unproven, not a silent skip. |
| 112-locked-items | A pause-then-inject case locks an item after the plan and during apply (as 128 does), and the item stays. The printed recovery step is parsed and run as printed. |
| 122-failed-move-leaves-no-copy | One real failed cross-volume move (destination on a full disk image), and no copy is left. The dead code at lines 20-26 is removed. |
| 70-content-blindness-traps | The FIFO scan's exit code is asserted (0), so a crash is not read as "did not block". The catalog contract says only what the scenario witnesses: sweep decides by name and does not block on a FIFO. The 3 unconditional unproven lines become notes. |
| 60-cross-volume-exdev | The retry at line 361 pins the documented exit and calls `assert_no_panic`. |
| 50-scale-large-and-zero-byte | Undo is followed by a full `snapshot_tree` comparison. |
| 20-unicode-nfc-nfd-collision | The check at line 84 looks in the group folder that exists today. |
| 50-scale-flat-10k-apply-timing | Line 185 is not an unconditional pass. |
| 60-exfat-same-device-rename-fallback | The image attaches with `-nobrowse`, so the volume does not show in Finder. |
| 81-unpack-quarantine | A measurement of a host tool is reported as a note, not as a pass. |
| 30-journal-truncation | A missing journal after apply is a FAIL, not unproven. The apply exit codes are asserted. |
