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
