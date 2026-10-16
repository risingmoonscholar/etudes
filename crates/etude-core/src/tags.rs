//! Finder tags as data: read, add to, and restore the tag attribute.
//!
//! Finder keeps a file's tags in one extended attribute,
//! `com.apple.metadata:_kMDItemUserTags`, holding a binary property list of
//! strings. A string is a tag name, optionally followed by a newline and a
//! colour number (`Red\n6`). Spotlight reads the same attribute, so writing it
//! is how a tag becomes visible in Finder.
//!
//! Only `--scheme tags` reads tag names, and only to add to them. Every
//! function here handles this one attribute. Nothing in this crate reads
//! Finder comments or lists attributes.
//!
//! Tag state is carried as the attribute's raw bytes wherever exactness
//! matters (the journal keeps the bytes it found and the bytes it wrote), and
//! as parsed strings only to decide what to add.

use std::io;
use std::path::Path;

/// The largest attribute value sweep will read or write. A value that large
/// is not a tag list a person made; the file is skipped by name instead.
pub const MAX_ATTR_BYTES: usize = 64 * 1024;
/// The most tags one file may carry after an add.
pub const MAX_TAGS_PER_FILE: usize = 200;
/// The longest tag name accepted from a caller, in characters.
pub const MAX_NAME_CHARS: usize = 100;

/// Check a caller-supplied tag name. A name goes into a plan, a journal and a
/// property list, so it must be plain: visible, no control characters, no
/// path separator, and not ending in space.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("a tag name cannot be empty".into());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "tag name {name:?} is longer than {MAX_NAME_CHARS} characters"
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(format!(
            "tag name {name:?} contains a control character or line break"
        ));
    }
    if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return Err(format!(
            "tag name {name:?} is not a plain name (no slash, backslash, . or ..)"
        ));
    }
    if name != name.trim() {
        return Err(format!("tag name {name:?} starts or ends with a space"));
    }
    Ok(())
}

/// The name part of a stored tag string: everything before the colour suffix.
pub fn name_of(entry: &str) -> &str {
    entry.split('\n').next().unwrap_or(entry)
}

/// Whether two tag names are the same tag. Compared without regard to case,
/// so asking for `work` where `Work` exists adds nothing twice.
pub fn same_name(a: &str, b: &str) -> bool {
    a == b || a.to_lowercase() == b.to_lowercase()
}

/// Parse the attribute's value into its tag strings, in order. `None` when the
/// value is not a binary property list holding only an array of strings; the
/// caller must treat that file's tag state as unreadable, never as empty.
pub fn decode(raw: &[u8]) -> Option<Vec<String>> {
    if raw.len() < 8 + 32 || raw.len() > MAX_ATTR_BYTES || &raw[..8] != b"bplist00" {
        return None;
    }
    let trailer = &raw[raw.len() - 32..];
    let offset_size = usize::from(trailer[6]);
    let ref_size = usize::from(trailer[7]);
    let count = usize::try_from(u64::from_be_bytes(trailer[8..16].try_into().ok()?)).ok()?;
    let top = usize::try_from(u64::from_be_bytes(trailer[16..24].try_into().ok()?)).ok()?;
    let table = usize::try_from(u64::from_be_bytes(trailer[24..32].try_into().ok()?)).ok()?;
    if !(1..=8).contains(&offset_size)
        || !(1..=8).contains(&ref_size)
        || count == 0
        || count > MAX_TAGS_PER_FILE + 1
        || top >= count
    {
        return None;
    }
    let table_end = table.checked_add(count.checked_mul(offset_size)?)?;
    if table < 8 || table_end > raw.len() - 32 {
        return None;
    }
    let body = &raw[..table];
    let offset_of = |index: usize| -> Option<usize> {
        let at = table + index * offset_size;
        let mut value = 0usize;
        for byte in &raw[at..at + offset_size] {
            value = value.checked_mul(256)?.checked_add(usize::from(*byte))?;
        }
        (8..table).contains(&value).then_some(value)
    };
    // Read a length that follows a marker: either in its low nibble, or, when
    // that nibble is 0xF, in a following integer object.
    let length = |at: usize, marker: u8| -> Option<(usize, usize)> {
        if marker & 0x0F != 0x0F {
            return Some((usize::from(marker & 0x0F), at + 1));
        }
        let int = *body.get(at + 1)?;
        if int >> 4 != 0x1 {
            return None;
        }
        let width = 1usize << (int & 0x0F);
        if width > 4 {
            return None;
        }
        let bytes = body.get(at + 2..at + 2 + width)?;
        let mut value = 0usize;
        for byte in bytes {
            value = value * 256 + usize::from(*byte);
        }
        Some((value, at + 2 + width))
    };
    let top_at = offset_of(top)?;
    let marker = *body.get(top_at)?;
    if marker >> 4 != 0xA {
        return None;
    }
    let (items, mut cursor) = length(top_at, marker)?;
    if items > MAX_TAGS_PER_FILE || items + 1 > count {
        return None;
    }
    let mut out = Vec::with_capacity(items);
    for _ in 0..items {
        let reference_bytes = body.get(cursor..cursor + ref_size)?;
        cursor += ref_size;
        let mut reference = 0usize;
        for byte in reference_bytes {
            reference = reference
                .checked_mul(256)?
                .checked_add(usize::from(*byte))?;
        }
        if reference >= count {
            return None;
        }
        let at = offset_of(reference)?;
        let marker = *body.get(at)?;
        let (size, start) = length(at, marker)?;
        match marker >> 4 {
            0x5 => {
                let bytes = body.get(start..start.checked_add(size)?)?;
                if !bytes.is_ascii() {
                    return None;
                }
                out.push(String::from_utf8(bytes.to_vec()).ok()?);
            }
            0x6 => {
                let bytes = body.get(start..start.checked_add(size.checked_mul(2)?)?)?;
                let units: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                    .collect();
                out.push(String::from_utf16(&units).ok()?);
            }
            _ => return None,
        }
    }
    Some(out)
}

/// Build the attribute value for a list of tag strings, as a binary property
/// list of strings. `None` when the list is too long to store.
pub fn encode(entries: &[String]) -> Option<Vec<u8>> {
    if entries.is_empty() || entries.len() > MAX_TAGS_PER_FILE {
        return None;
    }
    fn int_object(value: usize, out: &mut Vec<u8>) {
        if value < 0x100 {
            out.extend([0x10, value as u8]);
        } else {
            out.push(0x11);
            out.extend((value as u16).to_be_bytes());
        }
    }
    fn marker(kind: u8, size: usize, out: &mut Vec<u8>) {
        if size < 15 {
            out.push(kind | size as u8);
        } else {
            out.push(kind | 0x0F);
            int_object(size, out);
        }
    }
    let mut body = b"bplist00".to_vec();
    let mut offsets = Vec::with_capacity(entries.len() + 1);
    offsets.push(body.len());
    marker(0xA0, entries.len(), &mut body);
    // References are one byte wide: at most 201 objects.
    body.extend((1..=entries.len()).map(|index| index as u8));
    for entry in entries {
        offsets.push(body.len());
        if entry.is_ascii() {
            if entry.len() > 0xFFFF {
                return None;
            }
            marker(0x50, entry.len(), &mut body);
            body.extend(entry.as_bytes());
        } else {
            let units: Vec<u16> = entry.encode_utf16().collect();
            if units.len() > 0xFFFF {
                return None;
            }
            marker(0x60, units.len(), &mut body);
            body.extend(units.iter().flat_map(|unit| unit.to_be_bytes()));
        }
    }
    let table = body.len();
    let offset_size: usize = if table < 0x100 { 1 } else { 2 };
    for offset in &offsets {
        if offset_size == 1 {
            body.push(*offset as u8);
        } else {
            body.extend((*offset as u16).to_be_bytes());
        }
    }
    body.extend([0u8; 6]);
    body.push(offset_size as u8);
    body.push(1);
    body.extend((offsets.len() as u64).to_be_bytes());
    body.extend(0u64.to_be_bytes());
    body.extend((table as u64).to_be_bytes());
    (body.len() <= MAX_ATTR_BYTES).then_some(body)
}

/// Read a file's tag attribute: `Ok(None)` when it has none. A volume that
/// cannot hold attributes, an unreadable file or an oversized value is an
/// error, never "no tags".
pub fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
    crate::scan::observe_read("finder_tag_names", platform::read(path))
}

/// Replace the tag attribute with exactly these bytes.
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    platform::write(path, bytes)
}

/// Remove the tag attribute. A missing attribute is already removed.
pub fn remove(path: &Path) -> io::Result<()> {
    platform::remove(path)
}

#[cfg(target_os = "macos")]
mod platform {
    use super::MAX_ATTR_BYTES;
    use std::ffi::CString;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    /// Darwin `XATTR_NOFOLLOW`: act on the directory entry, never a link target.
    const XATTR_NOFOLLOW: i32 = 0x0001;
    /// Darwin `ENOATTR`: the attribute is absent.
    const ENOATTR: i32 = 93;
    /// Darwin `ERANGE`: the buffer was too small.
    const ERANGE: i32 = 34;
    const NAME: &std::ffi::CStr = c"com.apple.metadata:_kMDItemUserTags";

    unsafe extern "C" {
        fn getxattr(
            path: *const std::ffi::c_char,
            name: *const std::ffi::c_char,
            value: *mut std::ffi::c_void,
            size: usize,
            position: u32,
            options: i32,
        ) -> isize;
        fn setxattr(
            path: *const std::ffi::c_char,
            name: *const std::ffi::c_char,
            value: *const std::ffi::c_void,
            size: usize,
            position: u32,
            options: i32,
        ) -> i32;
        fn removexattr(
            path: *const std::ffi::c_char,
            name: *const std::ffi::c_char,
            options: i32,
        ) -> i32;
    }

    fn c_path(path: &Path) -> io::Result<CString> {
        CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::other("path contains a NUL byte"))
    }

    pub fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
        let path = c_path(path)?;
        for _ in 0..3 {
            // SAFETY: both strings are NUL-terminated and a null buffer of
            // length zero asks only for the size.
            let size = unsafe {
                getxattr(
                    path.as_ptr(),
                    NAME.as_ptr(),
                    std::ptr::null_mut(),
                    0,
                    0,
                    XATTR_NOFOLLOW,
                )
            };
            if size < 0 {
                let error = io::Error::last_os_error();
                return match error.raw_os_error() {
                    Some(ENOATTR) => Ok(None),
                    _ => Err(error),
                };
            }
            let size = size as usize;
            if size > MAX_ATTR_BYTES {
                return Err(io::Error::other("tag attribute is too large to handle"));
            }
            let mut buffer = vec![0u8; size];
            // SAFETY: the buffer owns `size` writable bytes.
            let got = unsafe {
                getxattr(
                    path.as_ptr(),
                    NAME.as_ptr(),
                    buffer.as_mut_ptr().cast(),
                    size,
                    0,
                    XATTR_NOFOLLOW,
                )
            };
            if got < 0 {
                let error = io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(ERANGE) => continue,
                    Some(ENOATTR) => return Ok(None),
                    _ => return Err(error),
                }
            }
            buffer.truncate(got as usize);
            return Ok(Some(buffer));
        }
        Err(io::Error::other(
            "tag attribute kept changing while it was read",
        ))
    }

    pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
        let path = c_path(path)?;
        // SAFETY: valid C strings and a slice that outlives the call.
        let rc = unsafe {
            setxattr(
                path.as_ptr(),
                NAME.as_ptr(),
                bytes.as_ptr().cast(),
                bytes.len(),
                0,
                XATTR_NOFOLLOW,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub fn remove(path: &Path) -> io::Result<()> {
        let path = c_path(path)?;
        // SAFETY: valid NUL-terminated strings.
        let rc = unsafe { removexattr(path.as_ptr(), NAME.as_ptr(), XATTR_NOFOLLOW) };
        if rc == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ENOATTR) {
            Ok(())
        } else {
            Err(error)
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::io;
    use std::path::Path;

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "Finder tags are a macOS feature",
        )
    }

    pub fn read(_: &Path) -> io::Result<Option<Vec<u8>>> {
        Err(unsupported())
    }

    pub fn write(_: &Path, _: &[u8]) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn remove(_: &Path) -> io::Result<()> {
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Produced by Python's plistlib for ["Red\n6"] and ["Work", "Zeta"].
    const ONE_RED: &[u8] = &[
        0x62, 0x70, 0x6c, 0x69, 0x73, 0x74, 0x30, 0x30, 0xa1, 0x01, 0x55, 0x52, 0x65, 0x64, 0x0a,
        0x36, 0x08, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x10,
    ];

    #[test]
    fn a_property_list_written_by_the_system_library_decodes() {
        assert_eq!(decode(ONE_RED), Some(vec!["Red\n6".to_string()]));
    }

    #[test]
    fn encoding_reproduces_the_system_library_bytes() {
        assert_eq!(encode(&["Red\n6".to_string()]).as_deref(), Some(ONE_RED));
    }

    #[test]
    fn a_longer_list_written_by_the_system_library_decodes_and_re_encodes_the_same() {
        let hex = "62706c6973743030a601020304050654576f726b565a6574610a326400430061006600e96365e5672c8a9e5f1028787878787878787878787878787878787878787878787878787878787878787878787878787878785f11012c7979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979797979790008000f0014001b0024002b00560000000000000201000000000000000700000000000000000000000000000186";
        let bytes: Vec<u8> = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap())
            .collect();
        let entries = decode(&bytes).expect("decodes");
        assert_eq!(entries.len(), 6);
        assert_eq!(entries[2], "Caf\u{e9}");
        assert_eq!(encode(&entries), Some(bytes));
    }

    #[test]
    fn names_round_trip_in_order_including_non_ascii_and_long_ones() {
        let entries: Vec<String> = vec![
            "Work".into(),
            "Zeta\n2".into(),
            "Caf\u{e9}".into(),
            "\u{65e5}\u{672c}\u{8a9e}".into(),
            "x".repeat(40),
            "y".repeat(300),
        ];
        let bytes = encode(&entries).expect("encodes");
        assert_eq!(decode(&bytes), Some(entries));
    }

    #[test]
    fn a_value_that_is_not_a_list_of_strings_is_refused_not_guessed() {
        assert_eq!(decode(b""), None);
        assert_eq!(decode(b"opaque-tag-bytes"), None);
        let mut cut = ONE_RED.to_vec();
        cut.truncate(cut.len() - 5);
        assert_eq!(decode(&cut), None);
        let mut wrong = ONE_RED.to_vec();
        wrong[8] = 0xd1; // a dictionary marker where the array should be
        assert_eq!(decode(&wrong), None);
    }

    #[test]
    fn names_are_validated() {
        assert!(validate_name("Work").is_ok());
        assert!(validate_name("Tax 2026").is_ok());
        for bad in ["", " x", "x ", "a/b", "a\\b", "..", "a\nb", "a\u{0}b"] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
        assert!(validate_name(&"n".repeat(MAX_NAME_CHARS + 1)).is_err());
    }

    #[test]
    fn the_name_is_the_part_before_the_colour() {
        assert_eq!(name_of("Red\n6"), "Red");
        assert_eq!(name_of("Work"), "Work");
        assert!(same_name("Work", "work"));
        assert!(!same_name("Work", "Works"));
    }
}
