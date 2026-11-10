# Releasing the file tools

Each tool has its own version and immutable tag: `sweep-vN`, `stash-vN`,
`unpack-vN`. A version in a manifest or a changelog is not a published release.

## Prepare a candidate

Use a branch and pull request, including documentation-only changes. Update the
affected CLI manifests, lockfile, README pins and changelog together. Keep the
README explicit about candidate status until publication is authorized.

Run the normal CI checks, capture fresh demo transcripts with
`scripts/capture-demos.sh` (it writes `site/transcripts.json` and the copy inside
`site/demo/index.html`), check the site with `python3 scripts/check-site.py`, then
install and exercise the candidate without changing a user's installation:

```sh
python3 -m unittest discover -s scripts -p 'test_check_release.py' -v
python3 scripts/check-release.py --candidate
```

The installation check uses fresh temporary installation roots and synthetic
files. It checks each binary's version, sweep apply/undo, stash/pop, and unpack
extraction plus refusal to overwrite an existing destination. Its journals use
a temporary state directory and a disposable supplied key. It is a smoke test,
not a substitute for the safety suite or independent review.

The ZIP fixture is produced by macOS `/usr/bin/zip`. This does not establish
compatibility with every ZIP writer: an in-memory Python `ZipFile.writestr`
member without Unix file-type bits was observed to be refused because the
typed listing did not supply a recognized member row. That refusal remains
unchanged in this candidate.

Keep the tested revision and logs. A local pass does not establish that an
independent reviewer completed successfully. If required review is unavailable,
preserve the candidate and hold certification and publication.

## The changelog entry

Each release has one entry on the site: `site/changelog/<tool>-<version>.json`.
It shows the same commands run on the build before and the build after, with
their real output and exit codes. One script writes it, so the page, the image
and the film text all come from the same runs. Nothing in it is typed by hand
except the date, label, headline, lead, the note under the next command, and
the film.

1. Write the fixture, `site/changelog/<tool>-<version>.fixture`: the shell lines
   that make the test folder, the commands, and which whole output lines the
   page shows. Copy `site/changelog/sweep-0.5.5.fixture`.
2. Run it on the candidate while the release is in review, for example
   `scripts/release-runs.py sweep 0.5.5 --before sweep-v0.5.4 --after HEAD`.
   The entry then says the after build is not tagged yet.
3. At tag time, run it again on the tag:

   ```sh
   scripts/release-runs.py sweep 0.5.5 --before sweep-v0.5.4 --after sweep-v0.5.5
   ```

   Commit the entry and its raw logs in `site/changelog/logs/sweep-0.5.5/`.
   `scripts/check-site.py` fails while an entry says "not tagged yet" after
   its tag exists.
4. Render the image of the entry, for sharing:

   ```sh
   scripts/render-entry-image sweep-0.5.5 ~/Desktop/sweep-0.5.5-entry.jpg
   ```

The script builds both refs in temporary worktrees and runs every command in
a fresh scratch folder with a fake `HOME`, a throwaway `ETUDE_STATE_DIR` and a
random `ETUDE_JOURNAL_KEY`, so it never touches the login keychain or a real
folder. `--target-dir DIR` keeps the Cargo build between runs. After changing
only the fixture's `show` lines, `scripts/release-runs.py sweep 0.5.5
--cuts-only` writes the entry again from the committed logs without building.

`scripts/check-site.py` derives every entry again from its fixture and raw logs
and fails on any difference, so an output line or exit code edited by hand in
the JSON is caught. It also checks that an untagged after build is an ancestor
of `HEAD`, and that a tagged one is the commit its tag names.
`scripts/render-entry-image` is local only (macOS, `swiftc`); CI does not run it.

## Publish and verify

After successful required review and the normal publication authorization,
finalize the release date and candidate wording through the PR. Recheck the
resolved release tree before tagging. Publish new tags and release notes; never
move an existing release tag to different code.

Verify the exact advertised pins from the public remote:

```sh
python3 scripts/check-release.py --published
```

This first requires all advertised tags to exist remotely. It then installs
from those tags using the committed lockfiles and runs the same smoke tests.
A missing tag, install failure, wrong binary version, or wrong observed effect
fails the check. The faster `--published --tags-only` mode checks existence
only and explicitly does not claim installation succeeded.

Retain the output with the release evidence. A failure means installation has
not been verified; do not replace it with a manifest comparison or a local build.
