# sweep

Organises the obvious and leaves the private alone.

```
$ sweep ~/Desktop

Scanned 95 items  ·  names, sizes and dates only  ·  no contents read

  Screenshots      34 files   named "Screenshot ..."
  Photos, Jul 28   27 files   camera names, taken within 3 days
  Installers        3 files   .dmg and .pkg
  acme              5 files   5 filenames contain "acme"

  Left alone       26 files
    14 look like personal records — sweep does not touch these
    12 no clear group

Nothing has been moved.  Nothing left this machine.
```

Every group states the signal that produced it, so you can check the reasoning
by eye. The "left alone" line is the point: sweep found documents that look like
tax and medical records and declined to organise them.

## Status: v0.1

Working: `sweep PATH`, `sweep apply --yes | --only NAME`, `sweep undo`,
`sweep forget`, `sweep verify`.

The undo journal is encrypted with XChaCha20-Poly1305 under a key held in your
login keychain. There is **no plaintext fallback**: if sealing is unavailable
sweep refuses rather than degrading.

Not implemented, and they say so rather than pretending:

- **`x` (extract files from a group)** during review.
- **Parsing PDFs, Office documents or archives.** `--inspect-content` reads
  plain text only, on purpose. See `docs/V03-CONTENT.md`.

## The privacy claims, and how to check them

| Claim | How you check it |
|---|---|
| Nothing leaves the machine | **`scripts/no-network-test.sh`** — runs the whole suite with `socket(2)` denied by the OS, after proving the sandbox actually denies. Plus `cargo deny check bans` and a symbol scan of the shipped binary. |
| The engine is auditable | `sweep-core` has **zero dependencies**, asserted by a test. No third-party code in the classification path. |
| No contents are read by default | sweep opens no file unless `--inspect-content` is passed **and** you answer `y` to a separate prompt. `--yes` does not cover it. |
| Reading can only make sweep act less | `cargo test --test content` — `inspection_never_creates_or_renames_a_group`. Content can move a file into "left alone" and nothing else. |
| Private files are not moved | `cargo test` — `sensitive_files_survive_a_full_apply_untouched` checks the filesystem, not the plan |
| The journal reveals nothing | `cargo test` — `no_filename_is_readable_in_the_written_journal` greps the bytes on disk |
| No filenames on screen when asked | `sweep --quiet` |

## Two design rules worth knowing

**sweep never coins a label the filesystem did not already contain.** A folder
named `Tax return 2024` is itself a disclosure — visible in Finder, indexed by
Spotlight, captured by every backup. Group names come only from tokens your own
filenames already carry. Files that look sensitive are never grouped at all.

**No general clustering.** Each detector is individually explainable and high
precision. A large "no clear group" count is a correct answer, not a failure.

## Documents

| File | Contents |
|---|---|
| `docs/CRITIQUE.md` | Risks in the concept, written before any code |
| `docs/SPEC.md` | v0.1 scope and the full CLI specification |
| `docs/THREAT-MODEL.md` | Assets, adversaries, controls, residual risks |
| `docs/PLAN.md` | Milestones and the acceptance tests that define done |

## Development

No real file is read during development. Everything is tested against a
synthetic adversarial fixture tree.

```sh
cargo test                        # 66 tests
scripts/no-network-test.sh        # the suite, with sockets denied by the OS
cargo run -p fixtures --bin mkfx -- /tmp/sweep-demo   # build a fake messy folder
cargo run -p sweep-cli --bin sweep -- /tmp/sweep-demo
```
