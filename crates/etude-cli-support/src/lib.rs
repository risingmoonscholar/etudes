//! Small presentation helpers shared by the command-line tools.

pub mod args;
pub mod contract;
pub mod envelope;
pub mod lock;
pub mod problems;
pub mod progress;
pub mod reason;

/// Whether help was asked for anywhere before a `--`. `sweep apply --help` is a
/// question, so it is answered instead of being refused as an unknown option.
pub fn wants_help(args: &[String]) -> bool {
    args.iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--help" || arg == "-h")
}

/// `text` in a form a shell reads back as one word, for commands the tools print
/// for someone to run next. None when it cannot be pasted safely: a control
/// character must not reach the terminal through a suggested command.
pub fn shell_word(text: &str) -> Option<String> {
    if text.is_empty() || text.chars().any(char::is_control) {
        return None;
    }
    let plain = |c: char| c.is_ascii_alphanumeric() || "_./~=:,@%+-".contains(c);
    if text.chars().all(plain) && !text.starts_with('-') && !text.starts_with('~') {
        return Some(text.to_owned());
    }
    Some(format!("'{}'", text.replace('\'', "'\\''")))
}

/// A name the caller typed, in full, for an error message. Control characters
/// are replaced so a hostile name cannot send escape codes to the terminal.
pub fn shown_text(text: &str) -> String {
    etude_core::shown_text(text)
}

/// A path the caller typed, for an error message. See [`shown_text`]. Names of
/// files the tool found by itself stay redacted; this is not for those.
pub fn shown(path: &std::path::Path) -> String {
    shown_text(&path.display().to_string())
}

/// `1 file`, `2 files`.
pub fn counted(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A path as one shell word for a printed next step, always quoted. A name that
/// holds a control character or invalid UTF-8 uses `$'...'` escapes, so the step
/// still works when pasted and no raw control byte reaches the terminal.
pub fn quoted_path(path: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    match std::str::from_utf8(bytes) {
        Ok(text) if !text.chars().any(char::is_control) => {
            format!("'{}'", text.replace('\'', "'\\''"))
        }
        _ => {
            let mut word = String::from("$'");
            for byte in bytes {
                match byte {
                    b'\'' => word.push_str("\\'"),
                    b'\\' => word.push_str("\\\\"),
                    0x20..=0x7e => word.push(char::from(*byte)),
                    _ => word.push_str(&format!("\\x{byte:02x}")),
                }
            }
            word.push('\'');
            word
        }
    }
}

/// An operating system error in a person's words: `permission denied`, without
/// the `(os error 13)` tail.
pub fn plain_io_error(error: &std::io::Error) -> String {
    let text = error.to_string();
    let text = text.split(" (os error").next().unwrap_or_default();
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Why a journal could not be used, without the internal prefix.
pub fn plain_journal_error(error: &etude_core::journal::JournalError) -> String {
    use etude_core::journal::JournalError as E;
    match error {
        E::Io(error) => plain_io_error(error),
        E::Malformed(what) | E::Seal(what) => (*what).to_owned(),
        E::NotFound => "no undo record was found".to_owned(),
    }
}

/// What to print when the journal key cannot be used. `--no-journal` is named
/// only by a command that takes it, because it removes undo and cannot help any
/// other command. Nothing has changed when this prints.
pub fn journal_key_problem(
    tool: &str,
    why: &dyn std::fmt::Display,
    supplied: bool,
    takes_no_journal: bool,
) -> String {
    let fix = if supplied {
        "Fix ETUDE_JOURNAL_KEY, or unset it to use the login keychain."
    } else {
        "Unlock the login keychain, or set ETUDE_JOURNAL_KEY to 64 hex digits."
    };
    let mut text =
        format!("{tool}: the journal key is unusable: {why}\n{fix} Nothing was changed.");
    if takes_no_journal {
        text.push_str("\nThe only alternative is --no-journal; it removes undo.");
    }
    text
}

/// Text for a journal that could not be written while acting.
pub fn journal_write_problem(error: &etude_core::journal::JournalError) -> String {
    format!(
        "cannot save the undo record in {}: {}",
        shown(&etude_core::journal::state_dir()),
        plain_journal_error(error)
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn printed_names_and_counts_read_correctly() {
        use std::path::Path;
        assert_eq!(super::shown(Path::new("/a/b\x1b[31m c")), "/a/b?[31m c");
        assert_eq!(super::counted(1, "file", "files"), "1 file");
        assert_eq!(super::counted(0, "file", "files"), "0 files");
        assert_eq!(super::counted(2, "entry", "entries"), "2 entries");
        assert_eq!(super::quoted_path(Path::new("/a b")), "'/a b'");
        assert_eq!(super::quoted_path(Path::new("/it's")), "'/it'\\''s'");
        assert_eq!(super::quoted_path(Path::new("/a\nb'c")), "$'/a\\x0ab\\'c'");
        let denied = std::io::Error::from_raw_os_error(13);
        assert_eq!(super::plain_io_error(&denied), "permission denied");
    }
}
