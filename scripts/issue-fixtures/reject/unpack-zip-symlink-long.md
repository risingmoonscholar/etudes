# unpack: an entry the type check cannot read is extracted instead of refused — and for zip, nothing else catches it

## What unpack promises

Before writing anything, unpack judges every member of an archive twice. First
lexically, on the name — traversal, absolute paths, depth. Then by type, reading
the mode string from a verbose listing — symbolic link, hard link, device node,
setuid bit. A member is refused if either judgment says so.

The type judgment is the one a name-only listing cannot make. It is the reason
unpack asks for a verbose listing at all.

## The bug

unpack reads the archive twice: once for a plain list of names, once for the
verbose typed listing. **The two readings are never compared.**

The verbose listing is parsed line by line. Any line the parser does not
recognise as a member row is skipped and forgotten
(`crates/unpack-cli/src/main.rs:497`, `:509`, `:538`, `:552`). That is correct
for headers and totals. It is also what happens to a member row the parser
cannot read.

The type judgment then runs only over rows that parsed
(`crates/unpack-cli/src/main.rs:237`). An entry present in the name listing but
absent from the parsed rows is never type-judged, is not refused, and is
extracted.

A different failure is handled correctly: if the verbose listing *command*
fails, unpack refuses the archive (`crates/unpack-cli/src/main.rs:247`).

So:

- Listing command fails → unpack **fails closed**. Correct.
- Listing command succeeds with output the parser cannot fully read → unpack
  **fails open**.

## Why this is worse for zip than for tar

unpack does not parse archives. It dispatches to the extractors the platform
ships. Those extractors have their own protections, and they are not equal.

**tar.** The platform extractor for the tar family documents, as
default behaviour, that leading slashes are stripped and that it refuses to
extract entries whose paths contain `..` or whose target directory would be
altered by a symbolic link. The flag that disables this is `-P`. unpack does not
pass it — the extraction call is the mode flag, the archive, `-C`, and the
destination (`crates/unpack-cli/src/main.rs:751`).

**zip.** The platform extractor for zip documents removing `../` components by default, with
`-:` as the flag that disables it. unpack passes only `-oq`
(`crates/unpack-cli/src/main.rs:733`), so that protection is active.

But that extractor's manual does not mention symbolic links anywhere. There is no
documented symlink containment to rely on, and measurement confirms there is
none in practice: a zip carrying a symbolic link, extracted with the same flags
unpack uses, produces the symbolic link on disk, pointing at an absolute path
outside the extraction directory, with no warning and no refusal.

The result is two layers of defence for tar and one for zip:

| path | unpack's type judgment | extractor default containment |
|---|---|---|
| tar family | yes | yes — documented, `-P` disables it |
| zip | yes | **none for symbolic links** |

The parse-skip bug above removes the first layer. For tar, the second layer
still holds. **For zip, nothing is left.**

That is what makes this a bug rather than a rough edge. The cross-check fix
matters everywhere, and it matters most for zip, because zip has no backstop.

## Two related gaps

**The symlink refusal is tested for tar only.** `tests/symlink_escape.rs` builds
its fixture as a tar-family archive. The zip path — the one without a backstop — has no
equivalent test.

**Nothing asserts the disabling flags stay absent.** The containment above holds
because `-P` and `-:` are not passed. No test asserts that. The protection rests
on nobody having written the flag, which is a property everything depends on and
nothing checks. Anyone adding a flag for an unrelated reason could remove it
without a single test failing.

## The success message overstates the check

On success unpack prints:

    Checked N paths before writing anything

N is the name-listing count (`crates/unpack-cli/src/main.rs:451`). Every path
did get the lexical check, so it is not false. But a reader hears both
judgments, and the type judgment may have covered fewer.

## Suggested fixes

1. **Cross-check the two readings.** Every name in the plain listing must have a
   corresponding typed row. A name without one could not be judged and should
   refuse the archive exactly as a declared hazard does, naming the entry.
2. **Test the zip symlink refusal** as `symlink_escape.rs` tests it for tar.
3. **Assert the extraction invocations.** A test that pins the exact arguments
   passed to each extractor, so that adding a containment-disabling flag fails
   the build.
4. **Report a count the second check actually reached.**

## On adding a dependency

Linking an archive-parsing library is the obvious way to get containment during
extraction. For the tar family it is not needed — the platform extractor already
enforces containment by default and unpack already declines to disable it.

For zip it is also not the answer. Linking a parser would move a large amount of
format-parsing code into unpack's own process, which is worse isolation than
dispatching to a separate program, and it would cost the property that unpack
takes no dependencies. The fix for zip is the cross-check above, not a library.

## Out of scope

An archive crafted so the verbose listing conceals what extraction then creates
is a different problem. unpack can only judge what the extractor reports.
Closing that requires containment during extraction, which the security notes
already state as out of scope for the formats that lack it.

This issue is about entries the listing did report and the parser could not read.

## A note on what the security notes should say

The notes currently describe extractor refusal as version-pinned platform
behaviour rather than a contract. For the tar family that undersells it: it is
documented default behaviour with a named flag that turns it off, and unpack
verifiably does not pass that flag. That is checkable by reading one line.

For zip it oversells it, because for symbolic links there is nothing there at
all.
