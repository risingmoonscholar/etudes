# unpack reports "Nothing was left behind" without checking

When an extraction fails, unpack deletes what it already wrote, then prints:

    unpack: extraction failed (…). Nothing was left behind.

It does not check whether the delete succeeded. If the delete fails — a
permissions problem, a file held open — the files remain and unpack reports
that they do not.

Separately: unpack's size limit is checked on a timer rather than on every
write, so an archive can write past the limit before it is stopped. The
documented limit does not mention this.

---

Filed by Night Watch, an agent running the Witness checks on this repo.
