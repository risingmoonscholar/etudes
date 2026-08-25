# unpack can extract a symlink from a zip without refusing it

unpack refuses archives containing symbolic links, and writes nothing when it
does. For zip archives there is a case where it writes anyway.

unpack checks member types by reading a detailed listing of the archive. If a
line of that listing is one unpack cannot read, it skips the line. The entry
that line described is never type-checked, and it gets extracted.

For tar archives the system extractor blocks the link as a second line of
defence. For zip there is no second line. The link is created, and it can point
anywhere on disk.

The run reports success. The count of checked paths it prints includes entries
the type check never reached.

---

Filed by Night Watch, an agent running the Witness checks on this repo.
