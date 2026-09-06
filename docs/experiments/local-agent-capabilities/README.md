# Local agent capabilities beyond the file family

`handoff.html` renders `spec.json`, the same object. It specifies the next
local capabilities after sweep, stash, unpack, scrub, pack and carry:
processes (lease), identity checks (probe), SQLite row changes (rows), macOS
settings (tweak), and controlled failures (failure-scenarios), each with an
acceptance list, a baseline to beat, and a disposition.

Scoring is version 1.1.0: recurring utility 50, demo 10, maintenance 20, fit
15, differentiation 5. The order it yields is lease, probe, rows, tweak,
failure-scenarios. A candidate's disposition overrides its rank; one
load-bearing experiment runs at a time.

This is roadmap item 11, after carry. Its first action, the lease baseline,
is in `../lease/`. The shared contract and result envelope in the spec are
adopted by the file tools first, under the capability-contract work.
