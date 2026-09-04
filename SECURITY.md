# Security

Written from the code, not from intent. Every claim here names the thing that
enforces it, so you can check rather than trust.

This replaces `docs/sweep/THREAT-MODEL.md`, which claimed `openat` with
`O_NOFOLLOW`, a path-scrubbing panic handler, `cargo vet`, reproducible builds
and published checksums. None of those exist, and none ever did. A privacy
tool asserting security machinery it does not have is worse than one that says
nothing, so that file is deleted rather than corrected.

## What holds

**No network.** `etude-core` declares zero dependencies, asserted by a test
against the manifest. `scripts/no-network-test.sh` runs the suite under
`sandbox-exec` with `socket(2)` denied, and proves the sandbox works first: a
control program that opens a TCP connection must succeed unsandboxed and be
denied under the profile. It is macOS-only, and exits `2` on any other host
rather than passing — a witness that quietly passes where it cannot observe is
worse than no witness.

The suite witnesses the code paths it exercises. It does not exercise the
`security`, `unzip`, `gunzip` or `tar` subprocess call sites.

**Contents are not read by default.** `--inspect-content` is off, and needs
consent at a prompt separate from `--yes`. What it reads can only move a file
*into* "left alone" — it never influences a destination. Plain text only; no
PDF, Office or archive parsing.

**Symlinks are not followed during traversal.** The scanner reads entry
metadata with `symlink_metadata`, which does not resolve the target
(`crates/etude-core/src/scan.rs`). A symlink is counted and skipped, never
walked. The marker check is deliberately written the same way, after two
earlier versions got it wrong in opposite directions — one dropped symlinked
markers, one resolved a target outside the scan root.

**Journals are encrypted.** XChaCha20-Poly1305 from RustCrypto, a 256-bit key
in the login keychain and never on disk, a fresh random 192-bit nonce per
write.

**`--json` discloses less than the human output.** For files that look like
personal records it carries counts by category and never the paths.

## unpack: what it refuses, and what that is worth

unpack dispatches to the system `unzip`, `tar` and `gunzip`. It parses no
archive format itself, which is its main defence and also the shape of its
limits.

**Refused at preflight, before anything is written**, each naming the member
and offering a recourse:

| refused | why |
|---|---|
| symlink members | a dispatched extractor cannot be stopped from writing *through* a link, and the link itself points wherever the archive says |
| hard link members | can name an inode outside the target, so writing "inside" changes data outside |
| device nodes, FIFOs, sockets | nothing an archive of files needs; a surface this tool will not open |
| setuid/setgid bits | an extracted file must not carry authority its extractor did not have |
| traversal, absolute and UNC paths | the classic escapes |
| a listing that reports names but not types | a check that cannot run is not a check that passed |

Measured, not assumed: before this existed, `unpack lone.zip` reported
*"Checked 2 paths before writing anything"* and then landed
`shortcut -> /etc/passwd` in the target, because the name-only listing it
used could not see a member's type. Both `bsdtar` and Info-ZIP refused to
write *through* such a link on the machine this was measured on — but that
is version-pinned platform behaviour, not a contract this tool offers, and
the link itself was still delivered.

**What this does not cover, stated plainly:**

- **A listing that lies about types.** Listing and extraction are separate
  parses and can disagree. An archive that conceals a symlink from
  `tar -tvf` could still cause one to be created. Preventing that needs
  containment *during* extraction — libarchive's secure-extraction flags, or
  a sandboxed extractor — which is a different tool than the one that parses
  nothing.
- **A hostile process running as you.** unpack copies the archive into a
  freshly created 0700 directory and runs every listing and the extraction
  against that copy, so replacing or rewriting the original path after the
  preflight cannot change what is extracted. That closes the attack from
  another *user*. It does not close it from a process running under your own
  account: 0700 keeps others out and never you, and such a process can list
  the temporary directory and find the copy whatever it is named. A random
  name stops it being predicted, not being found. Closing this needs a handle
  with no name to find -- an unlinked descriptor -- which is a different
  design than a private copy, and this tool does not have it. If another
  process is already running as you, it does not need unpack to do anything.

- **Nested archives.** A zip inside a zip is just a file here; unpack does
  not recurse, so the bound applies per invocation, not to what you unpack
  next.
- **CPU and time.** The write bound is on bytes landing on disk. A small
  archive that decompresses slowly is not bounded by it.
- **Parser vulnerabilities in the system tools.** Dispatching means their
  bugs are reachable. Keep macOS patched; unpack does not run privileged.

The honest claim is: *unpack refuses every dangerous member its listing
declares, writes nothing when it cannot judge one, and extracts the same
bytes it judged unless something is already running as you.* Not "safe
unarchive", and deliberately not "the checked bytes are the bytes
extracted" without that last clause -- six review rounds went into finding
out that the shorter sentence was not true.

**The attacks are public, on purpose.** `stress/scenarios/` ships in the
clone and builds its hostile archives at run time -- a symlink pointing at
`/etc/passwd`, a setuid bit, a FIFO, member names containing spaces,
newlines, `->`, and strings that mimic a mode column. None of it is novel:
Zip Slip, tar symlink traversal and setuid-in-archive are decades old and
sit in every security curriculum and CVE record. Publishing the scenarios
withholds nothing from anyone and is what lets a reader check the refusal
rather than trust it. A security claim whose tests are private is a claim,
not evidence.

The fixtures are generated rather than committed, and the reasons are
hygiene and reviewability, NOT secrecy -- an earlier draft of this section
implied otherwise and was wrong. Anyone who runs the script gets the same
archive a committed one would hand them. What differs is that a script is
readable where a blob is not, and that a committed malicious archive lands
on every clone's disk, gets indexed, and can trip endpoint scanners on
machines belonging to people who never ran the tests. Neither reason is a
security property.

## What does not hold, and is not meant to

**Ciphertext length leaks coarsely.** Journals are padded to the next 4 KiB
bucket. The test is named `padding_hides_size_within_bucket_not_across`, which
is exactly the claim: within a bucket, nothing; across buckets, an observer
learns roughly how much work was done. File mtimes leak activity regardless.

**Anyone with your login session has the key.** The keychain protects against
someone reading the disk, not against code running as you. Nothing here
defends against a local attacker who already has your account.

**No supply-chain verification.** `cargo deny` bans dependency categories in
CI. There is no `cargo vet`, no reproducible build, no published checksum, and
no signature. If you install from git you are trusting this repository and
whatever `cargo` resolves.

**No protection against a filesystem race.** A file that changes between plan
and apply is handled — the apply re-checks — but this is not a hardened
against-an-adversary property, and the stress suite records where it could not
be proven on this machine rather than claiming it passed.

**Refusals are convenience gates, not boundaries.** `--inspect-content` and
`review` require a TTY. That stops accidental non-interactive use; it does not
stop a process driving a pty.

**Machine-wide reads answer only to a person at a terminal.** The operating
rule, applied wherever a command's reach exceeds the folder it was pointed
at: refuse non-interactive callers, and state the reasoning in the refusal.
Currently gated this way:

| surface | what it reveals or does |
|---|---|
| `stash status --all --paths` | every stashed folder's full path |
| `journal-dump` | the decrypted pathname history of every move |
| `sweep --inspect-content` | consent to reading contents at all |
| `sweep review` | interactive renaming |
| `sweep forget`'s key destruction | the key stash also relies on |

A TTY gate is a default-flip, not a boundary: a process driving a pty walks
through, deliberately and auditably. `stash status --all` without `--paths`
stays agent-callable -- ids, ISO deadlines and redacted roots are enough to
build a schedule against, which is the intended delegation surface.

## Checking any of this

```sh
cargo test --all                # the suite
scripts/no-network-test.sh      # the same suite, sockets denied by the OS
sweep verify                    # what is compiled in, on this machine
bash stress/run.sh              # the adversarial harness
```

## Reporting

Open an issue. There are no users to coordinate a disclosure with, and
pretending otherwise would be theatre.
