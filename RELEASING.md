# Releasing the file tools

Each tool has its own version and immutable tag: `sweep-vN`, `stash-vN`,
`unpack-vN`. A version in a manifest or a changelog is not a published release.

## Prepare a candidate

Use a branch and pull request, including documentation-only changes. Update the
affected CLI manifests, lockfile, README pins and changelog together. Keep the
README explicit about candidate status until publication is authorized.

Run the normal CI checks, capture fresh demo transcripts, then install and
exercise the candidate without changing a user's installation:

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
