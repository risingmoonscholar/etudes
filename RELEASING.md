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
except the label, headline, lead, the note under the next command, and the
film.

The entry does not say whether the release is tagged, or give a date.
`scripts/build-site.py` reads both from git each time it builds:

- No `<tool>-v<version>` tag: the entry is a release candidate. The changelog
  shows "Release candidate" and no date, and the footer says "not tagged yet".
- The tag names the commit the after build ran at, or a commit with the same
  source for that tool (its crate, the crates it uses by path, the workspace
  `Cargo.toml` and `Cargo.lock`): the entry is tagged. The changelog shows the
  tag's date, and the footer names the tag.
- The tag names a commit whose source for that tool differs: the site does not
  build until the entry is run again on the tag.

Steps:

1. Write the fixture, `site/changelog/<tool>-<version>.fixture`: the shell lines
   that make the test folder, the commands, and which whole output lines the
   page shows. Copy `site/changelog/sweep-0.5.5.fixture`.
2. While the release is in review, run it on the candidate, for example
   `scripts/release-runs.py sweep 0.5.5 --before sweep-v0.5.4 --after HEAD`.
   Write the label, headline, lead and next note in the JSON. Commit the entry
   and its logs in `site/changelog/logs/sweep-0.5.5/`.
3. Tag. If the tag is on the commit the entry ran at, or the tool's source did
   not change after it, nothing else is needed: the site shows the release as
   tagged, with the tag's date. If the source changed, run it again on the tag
   and commit the result before publishing:

   ```sh
   scripts/release-runs.py sweep 0.5.5 --before sweep-v0.5.4 --after sweep-v0.5.5
   ```

4. Render the image of the entry, for sharing:

   ```sh
   scripts/render-entry-image sweep-0.5.5 ~/Desktop/sweep-0.5.5-entry.jpg
   ```

How the runs are kept away from your files and your keychain:

- The script refuses, before it builds anything, a sweep or stash ref that does
  not read `ETUDE_JOURNAL_KEY` (anything before 0.5.3). Those builds fetch
  their journal key from the login keychain with `/usr/bin/security`.
- Every command runs under `sandbox-exec` with a profile that denies running
  `/usr/bin/security` and denies writing anywhere outside the run's scratch
  folder. A `security` that only refuses is also first on `PATH`.
- Each run has a fake `HOME`, a throwaway `ETUDE_STATE_DIR` and a random
  `ETUDE_JOURNAL_KEY`.
- The builds use temporary worktrees, removed after. `--target-dir DIR` keeps
  the Cargo build between runs.

The logs are each command's output with one change: the scratch folder's path,
different on every run and machine, is written as `<scratch>`.
`scripts/check-site.py` fails on a log that still names a machine's folder.

After changing only the fixture's `show` lines or labels,
`scripts/release-runs.py sweep 0.5.5 --cuts-only` writes the entry again from
the committed logs without building.

`scripts/check-site.py` derives every entry again from its fixture and logs
and fails on any difference, so an output line or exit code edited by hand in
the JSON is caught. It also checks that an untagged after build is an ancestor
of `HEAD`. `scripts/render-entry-image` is local only (macOS, `swiftc`); CI does
not run it.

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
