# Carry verified copy protocol

Version 1 defines a proposed macOS operation that copies a regular file or complete directory tree and verifies the result. A successful copy retains the source, preserves any existing destination, and reports data fidelity separately from metadata fidelity. These requirements do not add a carry executable or change the existing tools.

## Existing destinations and copy scope

Carry must resolve the destination parent without following a symlink through any source or destination component. Version 1 supports regular files and directories. It must refuse symlinks, devices, FIFOs and sockets anywhere in the selected tree. Filesystem name matching determines whether a destination already exists; case folding and Unicode equivalence must not become accidental replacement rules.

A destination with different bytes, member paths or member types is refused without modification. A directory transaction covers the whole tree: carry must never merge into an existing directory or fill only its missing members. An existing destination is `nothing_to_do` only after a fresh read verifies its complete bytes and length against a stable source and required security metadata matches. For a tree, the relative-path and type manifest, every file stream, and every required security field must match; an extra or missing member is a differing destination. Same inode, matching size, matching timestamps, or an earlier digest alone is insufficient. A same-inode source and destination may qualify as this verified no-op, but carry must never rewrite either name. Ordinary metadata differences are reported individually; carry does not repair an existing file in place. Unreadable or changing destinations cannot qualify as identical.

Carry must never request move, source unlink, destination unlink, or replacement flags. It may remove only private temporary objects it created and still identifies as its own. It must not delete a published copy as rollback. Network transport and cross-machine authentication are outside version 1. A local receipt cannot prove that a remote peer received the same bytes.

## Source capture and mutation detection

Before copying, carry must open the source without following links, confirm the supported type from the descriptor, and capture device, inode, length, modification time, change time and the metadata covered below. For a directory, it must enumerate a complete manifest of relative paths and types without following links, capture the identity of each member, and retain directory descriptors while resolving descendants. It must retain that descriptor while copying. Replacing the pathname must not substitute a new file's bytes or provenance.

The implementation must compare the source identity and captured state after copying, then reread the complete source through that descriptor and compare it with the private destination. For a tree, it must enumerate again and compare every member identity, type, stream and required metadata field with the original manifest; additions and removals are source changes even when all surviving file bytes match. Immediately before publication it must check the source descriptor, source pathname identity and required metadata again. Append, truncate, same-length rewrite, pathname replacement, an added or removed tree member, and a change to security metadata must refuse publication. Directory identities and membership must be checked at the final boundary as well as individual files. Read or identity-check failure is an error, not evidence of stability.

A descriptor pins identity; it does not freeze contents. Matching boundary checks cannot prove that an adversarial writer changed and restored every observed property between checks. The source must provide a consistent read view and sufficient change observability for the supported concurrency model. If that cannot be established, source stability is `unproven` and carry must refuse publication. File size or timestamps alone cannot satisfy this requirement, especially on a filesystem with coarse timestamp resolution. The result must state the checked boundaries and must not claim continuous immunity to writes after the last check.

## Destination verification and publication

Carry must create private staging beside the destination with exclusive creation, record its identity, and copy into that new object. A tree must be built wholly under one private staging root, with no member published early. Required directory permissions and ACLs must be applied without exposing a partially built tree. It must close or flush the writer and reopen staging independently for a complete read. Data fidelity is `pass` only when the complete path/type manifest, each required stream length, and every byte match the verified source view. A count of files or a matching root digest without individual readback cannot establish this claim. SHA-256 may identify the compared synthetic or expressly requested artifact, but a digest does not replace the complete comparison or prove metadata equality.

After applying metadata, carry must reread each required field and repeat the final data and source checks. Publication must publish the complete file or staging tree in one filesystem operation that atomically refuses an existing destination. An earlier absence check followed by ordinary replacement-capable `rename` is prohibited. If the destination appears during copying, carry must refuse and leave it intact. A filesystem lacking a proven exclusive publication operation is unsupported for publication.

After publication, carry must reopen the destination, confirm it names the verified staging identity, and reread its complete manifest, bytes and metadata. All tree members must be present with their verified identities and no unexpected additions. A failure here is `incomplete`, with the published copy retained and its location disclosed for inspection. It must never become a success merely because the earlier staging read passed. Flush results and crash durability are separate claims: ordinary `fsync` is not proof of survival through device power loss.

## Metadata fidelity and unsupported storage

Metadata results must name the field, whether it was present on the source, whether transfer was attempted, and a `pass`, `fail` or `unproven` comparison. Absence requires an actual query; a zero counter is not evidence of absence. Source contents, attribute values, identities of ACL principals, and environment values must not appear in default receipts.

Required security fidelity covers access permissions, owner and group meaning, ACL permissions, execution restrictions, and quarantine or other security provenance attributes present on the source. If a field cannot be interpreted, stored or read back without weakening those restrictions, carry must refuse publication. It must not guess how another filesystem or account namespace interprets ownership. Unknown attributes must be classified before publication; an unknown field cannot silently become disposable metadata.

Ordinary metadata covers modification and creation times, Finder tags and comments, and declared application attributes. Unsupported fields may accompany a completed data copy only with explicit per-field loss or rounding and `metadata_fidelity: fail`; data fidelity can still be `pass`. Resource forks contain additional data and must be compared separately as a required data stream when present. Sparse allocation and compression layout are not byte fidelity, and must be described separately if reported. Access time changes caused by reading are disclosed and excluded from mutation comparison, not restored by writing to the source.

macOS can store attributes on exFAT through AppleDouble companions. The representation must be identified and read back rather than inferred from the filesystem name. If preserving a required stream or security attribute needs multiple filesystem objects, version 1 must refuse unless their complete publication is proven atomic with no replacement. It must not publish the main file and promise to add its security companion afterward. This restriction differs from publishing a complete staged directory in unpack.

## Outcomes and recovery

Use the established result envelope and a separately versioned carry declaration. `done` requires verified data, required security fidelity, and publication readback. `nothing_to_do` requires verified existing data and security metadata without writes. `refused` covers a differing destination, detected source change, unsupported publication or required fidelity, and unproven source stability. `error` covers failed reads, writes or verification before publication. `incomplete` covers failed verification after publication or unresolved cleanup. Exit classes remain 0 completed, 1 nothing to do, 2 refused and 3 error; incomplete uses 3.

Before publication, failure or cancellation removes only owned staging and reports any cleanup failure. After publication, neither cancellation nor a failed check may remove the output. A retry must compare an existing output from scratch. Interrupted staging may be reported for explicit recovery; a later invocation must not sweep unknown temporary files or adopt an old receipt as current verification.

Receipts separate attempted, observed, verified, failed and unproven reads. They identify source checks, destination reads, publication, metadata comparisons and cleanup, with narrow claim verdicts. Default disclosure contains categories, counts, fidelity reasons and an operation ID; paths are disclosed only when needed to identify input, output or stranded staging. Raw contents, metadata values, secrets and unsolicited content fingerprints are excluded.

## Required fixtures and demonstration

The versioned [fixture manifest](fixtures-v1.json) records expected outcomes, not measured results. Generate fixtures in disposable directories and record which intended properties were successfully created. Failure to create a property makes its case `unproven`. Run the matrix on each supported macOS version and filesystem combination; unsupported combinations must produce the specified refusal rather than a downgraded success.

| Fixture | Required result |
| --- | --- |
| Empty, binary and multi-buffer files; nested trees and empty directories | Complete destination reread equals every source byte |
| Identical existing file and same-inode alias | Verified no-op; no source or destination writes |
| Different existing file or tree; destination appearing before publish | Refusal; existing identity and bytes intact |
| Case-folded and NFC/NFD destination collision with differing data or members | Filesystem collision refused without replacement |
| Append, truncate, same-length rewrite and restored-size rewrite during copy | Mutation detected before publication |
| Added or removed tree member, source path replaced or changed to a symlink | Refusal; replacement is neither read as the source nor modified |
| Quarantine, ACL, permissions or provenance changed during copy | Refusal; required security state never downgraded |
| APFS source to APFS, exFAT and a read-only destination | Exact per-field comparisons or explicit unsupported refusal |
| Resource fork and AppleDouble representation | Both data streams verified; required multi-object publication refused unless atomicity is proven |
| Rounded timestamps or unsupported ordinary metadata | Data pass and named metadata failure, never overall metadata pass |
| Corrupted staging and corrupted published destination | Pre-publication error or post-publication incomplete, respectively |
| Disk full, permission failure and interruption at copy, verify and publication boundaries | No input or pre-existing output deleted; owned staging cleanup or an explicit stranded location |

The demonstration must show successful file and tree copies, identical file and tree no-ops, differing-file and differing-tree refusals, file and tree mutation refusals, and a filesystem fidelity limit. Every byte in successful readback must match; no differing destination may be replaced; every injected source mutation must be detected before publication. The independent comparison must reject a deliberately corrupted destination and a dropped required security attribute. Any required assertion that fails or remains unproven blocks a success claim for that combination. No speed threshold is set; duration, bytes compared and interruption points must be recorded without a performance promise.

## Sources and existing measurements

- [Apple copyfile manual](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/copyfile.3.html): data, POSIX metadata, ACL and xattr transfer are separate flags; move and unlink flags delete names; descriptor copying and AppleDouble packing are distinct operations. A successful call does not supply this protocol's readback evidence.
- [Apple open manual](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/open.2.html) and [fstat manual](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fstat.2.html): descriptor access, exclusive creation, link handling and identity fields used for capture and comparison.
- [Apple rename manual](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/rename.2.html): ordinary rename can remove an existing destination. [Exclusive renaming capability](https://developer.apple.com/documentation/foundation/urlresourcevalues/volumesupportsexclusiverenaming) documents that `RENAME_EXCL` support depends on the volume.
- [Apple getxattr manual](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/getxattr.2.html) and [fsetxattr manual](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsetxattr.2.html): descriptor-based attribute reads and writes, absent attributes, unsupported storage and readback errors.
- [Apple fsync manual](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html): host flushing and device power-loss durability are different guarantees.
- [NIST Secure Hash Standard](https://csrc.nist.gov/pubs/fips/180-4/upd1/final): SHA-256 specifies a content digest, not filesystem metadata or publication semantics.

The existing [unpack quarantine findings](../../../README.md#download-quarantine-on-extracted-archives) report macOS 27.0.1 attribute propagation and exact exFAT readback through AppleDouble companions. They establish a storage case for this fixture matrix; they do not establish a carry implementation, atomic publication of a standalone file and companion, or a new measurement of these requirements.
