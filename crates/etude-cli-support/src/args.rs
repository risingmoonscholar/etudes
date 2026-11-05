//! The command line, read once and in one place.
//!
//! `std::env::args()` panics on an argument that is not valid UTF-8. The tools
//! read `args_os()` here instead and refuse such an argument with exit 2 and
//! the `invalid_arguments` reason. A file name with those bytes cannot exist
//! on APFS, and no option takes a value that is not text, so there is nothing
//! to guess at: the person is told which argument it was.

use std::ffi::OsString;
use std::process::ExitCode;

/// The first argument that is not valid text.
#[derive(Debug, PartialEq, Eq)]
pub struct BadArgument {
    /// 1-based, not counting the program name.
    pub position: usize,
    /// The argument for a message: invalid bytes and control characters are
    /// shown as `?`, so no raw byte reaches the terminal.
    pub shown: String,
}

/// An argument for a message, with invalid bytes and control characters as `?`.
fn shown_os(arg: &OsString) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::new();
    for chunk in arg.as_os_str().as_bytes().utf8_chunks() {
        out.push_str(&crate::shown_text(chunk.valid()));
        if !chunk.invalid().is_empty() {
            out.push('?');
        }
    }
    out
}

/// Every argument as text, or the first one that is not.
pub fn from_os(args: impl IntoIterator<Item = OsString>) -> Result<Vec<String>, BadArgument> {
    args.into_iter()
        .enumerate()
        .map(|(i, arg)| {
            arg.clone().into_string().map_err(|_| BadArgument {
                position: i + 1,
                shown: shown_os(&arg),
            })
        })
        .collect()
}

/// The sentence for a bad argument.
pub fn message(tool: &str, bad: &BadArgument) -> String {
    format!(
        "{tool}: argument {} ({}) has bytes that are not valid text. Nothing was changed. Type it again without them.",
        bad.position, bad.shown
    )
}

/// The process arguments for `tool`, past the program name.
///
/// On a bad argument this has already begun and finished the result envelope,
/// printed the message, and set `invalid_arguments`; the caller returns the
/// code it gets back.
pub fn collect(tool: &str, version: &str) -> Result<Vec<String>, ExitCode> {
    let raw: Vec<OsString> = std::env::args_os().skip(1).collect();
    from_os(raw.iter().cloned()).map_err(|bad| {
        // Only the operation name and `--json` are read from this copy.
        let lossy: Vec<String> = raw
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        crate::envelope::begin(tool, version, &lossy);
        eprintln!("{}", message(tool, &bad));
        crate::envelope::reason(crate::reason::Reason::refusal(
            crate::reason::Code::InvalidArguments,
            None,
        ));
        let code = ExitCode::from(2);
        crate::envelope::finish(code);
        code
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    fn os(bytes: &[u8]) -> OsString {
        OsString::from_vec(bytes.to_vec())
    }

    #[test]
    fn text_passes_through() {
        let got = from_os([os(b"scan"), os("café".as_bytes())]).unwrap();
        assert_eq!(got, ["scan", "café"]);
    }

    #[test]
    fn first_bad_argument_is_named_without_its_bytes() {
        let bad = from_os([os(b"ok"), os(b"bad\xff\xfe\x1b[2Jend"), os(b"\xff")]).unwrap_err();
        assert_eq!(bad.position, 2);
        assert_eq!(bad.shown, "bad???[2Jend");
        let text = message("sweep", &bad);
        assert!(text.is_ascii() && !text.contains('\x1b'));
    }
}
