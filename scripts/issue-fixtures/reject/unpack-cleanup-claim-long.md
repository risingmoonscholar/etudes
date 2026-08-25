# unpack: the size limit can be overshot, and cleanup can fail silently

This is not a hole in the size limit. The limit works, and archives that try to
fill a disk are stopped. This issue is about two known slacks in it that are
not written down where someone relying on the limit would look.

## The limit overshoots by design

unpack caps how much an extraction may write. Three of the four supported
formats are extracted by a system process that writes files directly, so unpack
cannot wrap the write stream. It measures the destination on an interval
instead, and stops the extraction when the total passes the budget
(`crates/unpack-cli/src/main.rs:688`, `:710`).

Polling means whatever lands between two measurements is already on disk before
the check runs. The source says so plainly at the polling loop. The
documentation does not.

The design is defensible — for a child process writing files directly, an
interval check is the available mechanism. But a reader who sees a size limit
reasonably expects the limit to be the ceiling, and here the ceiling is the
limit plus one interval of writing.

**Suggested fix:** state the bound where the limit is claimed, in terms a
reader can use — the poll interval, and therefore the worst-case overshoot at a
given write rate. A limit with a stated slack is a specification. A limit with
an unstated slack is a surprise.

## Cleanup discards its own errors, then reports success

When an extraction fails, or when it breaches the budget, unpack removes what
landed. Both cleanup sites discard the result of the removal
(`crates/unpack-cli/src/main.rs:392`, `:400`).

The failure path then prints:

    unpack: extraction failed (…). Nothing was left behind.

That sentence is printed unconditionally, after an operation whose result was
thrown away. If the removal fails — a permissions problem, a file held open, a
read-only parent — partial output stays on disk and unpack reports that it does
not.

This is the more serious of the two, because it is a claim rather than a
limitation. The tool tells the user something it did not check.

**Suggested fix:** either check the removal and report honestly when it fails,
naming what remains and where, or change the sentence to state what unpack
actually knows — that removal was attempted. The first is better. A user
cleaning up after a failed extraction needs to know whether there is anything
to clean up.

## Why both matter together

unpack's value is that a user can point it at an untrusted archive and trust
what it says afterwards. Both items here are places where the tool's account of
its own behaviour is more confident than its behaviour warrants — one by
omission, one by assertion.

Neither is a reason to distrust the size limit. Both are reasons the limit
should describe itself accurately.
