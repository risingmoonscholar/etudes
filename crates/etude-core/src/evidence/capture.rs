//! Screen-capture recognition, and how sure sweep is about it.
//!
//! Sources (CONTRIBUTING.md requires them for platform claims):
//!
//! - The localized words macOS starts a capture file name with, in every
//!   language it ships: `/System/Library/CoreServices/screencaptureui.app/
//!   Contents/Resources/Localizable.loctable`, keys `Screenshot` and
//!   `Screen Recording`, read directly with `plutil` on macOS 27.0.1
//!   (26A434). Primary. The table is generated from it by
//!   `scripts/gen-capture-names.sh`; see `capture_names.rs`.
//! - The output formats: `man screencapture` lists `-t` with `png` (the
//!   default), `pdf`, `jpg`, `tiff` "and other formats", and `-v` for video.
//!   Primary. `heic` comes from the English rule sweep already shipped.
//!
//! Nothing here touches the filesystem. Recognition is a pure function of
//! the file name, so it reads no attributes and no contents.

use super::capture_names::{RECORDING_WORDS, STILL_WORDS};
use std::path::Path;

/// What a screen capture holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureKind {
    /// A still screenshot.
    Still,
    /// A screen recording.
    Recording,
}

/// Why sweep believes a file is a screen capture.
///
/// Declared strongest first, and `Ord` follows the declaration, so the
/// weakest basis is the greatest. A group of captures reports the `max` of
/// its files' bases: the output must not claim more than its weakest file
/// supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CaptureBasis {
    /// macOS marked the file as a screen capture with an attribute.
    OsAttribute,
    /// The name starts with the word macOS itself uses, in some language.
    OsName,
    /// The name matches the English rule sweep used before it knew the OS
    /// words. It also covers CleanShot, which is not part of macOS.
    KnownName,
}

/// A file recognised as a screen capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Capture {
    pub kind: CaptureKind,
    pub basis: CaptureBasis,
}

/// Whether a file with this extension can be a screen capture, and which
/// kind. Case does not matter, as with every extension sweep compares.
///
/// The list is what `screencapture` can write, plus `heic`, which the
/// English rule already accepted. An extension outside it is never a
/// capture, whatever the file is called: a `Screenshot 1.txt` is a text
/// file.
pub fn capture_kind_for_extension(ext: &str) -> Option<CaptureKind> {
    match ext.to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "heic" | "tiff" | "tif" | "gif" | "pdf" | "bmp" => {
            Some(CaptureKind::Still)
        }
        "mov" => Some(CaptureKind::Recording),
        _ => None,
    }
}

/// Recognise a screen capture from its file name alone.
///
/// The OS words are compared as exact code points and exact case, the way
/// macOS writes them. Nothing is stripped or normalised: the Dutch word
/// carries a soft hyphen (U+00AD) and the French word a typographic
/// apostrophe (U+2019), and a name typed without them was not written by
/// macOS.
pub fn recognise_by_name(file_name: &str) -> Option<Capture> {
    // The same split the scan uses, so `.png` alone has no extension here
    // either.
    let path = Path::new(file_name);
    let ext = path.extension()?.to_str()?;
    let kind = capture_kind_for_extension(ext)?;
    let stem = path.file_stem()?.to_str()?;

    // The attribute check (`CaptureBasis::OsAttribute`) belongs before this
    // one, because it is stronger. It needs a filesystem probe, so it lives
    // with the caller that has the path, not in this pure name check.

    let words = match kind {
        CaptureKind::Still => STILL_WORDS,
        CaptureKind::Recording => RECORDING_WORDS,
    };
    if words.iter().any(|word| starts_os_name(stem, word)) {
        return Some(Capture {
            kind,
            basis: CaptureBasis::OsName,
        });
    }

    if is_known_name(file_name, ext) {
        return Some(Capture {
            kind: CaptureKind::Still,
            basis: CaptureBasis::KnownName,
        });
    }
    None
}

/// macOS names a capture as the word, a space, then the date. Requiring the
/// digit keeps an ordinary title that happens to start with the word, such
/// as "Screenshot ideas.png", out. The space is optional because some
/// languages (Chinese, Thai) write the date straight after the word.
fn starts_os_name(stem: &str, word: &str) -> bool {
    let Some(rest) = stem.strip_prefix(word) else {
        return false;
    };
    let rest = rest.strip_prefix(' ').unwrap_or(rest);
    rest.as_bytes().first().is_some_and(u8::is_ascii_digit)
}

/// The English rule sweep shipped before P5, reproduced exactly so no file
/// it grouped changes group: the name starts with `screenshot`,
/// `screen shot` or `cleanshot` in any ASCII case.
///
/// It keeps its own narrower extension list (png, jpg, jpeg, heic), not the
/// whole capture set. Widening it would change which files today's builds
/// group, and that is not this rule's job; the OS words cover the other
/// formats.
fn is_known_name(file_name: &str, ext: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    (lower.starts_with("screenshot")
        || lower.starts_with("screen shot")
        || lower.starts_with("cleanshot"))
        && matches!(
            ext.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "heic"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    const STILL_TAIL: &str = " 2026-11-12 at 09.41.12.png";
    const RECORDING_TAIL: &str = " 2026-11-12 at 09.41.12.mov";

    fn capture(kind: CaptureKind, basis: CaptureBasis) -> Option<Capture> {
        Some(Capture { kind, basis })
    }

    #[test]
    fn every_still_word_is_recognised_by_the_os_name() {
        assert!(!STILL_WORDS.is_empty());
        for word in STILL_WORDS {
            let name = format!("{word}{STILL_TAIL}");
            assert_eq!(
                recognise_by_name(&name),
                capture(CaptureKind::Still, CaptureBasis::OsName),
                "{name:?}"
            );
        }
    }

    #[test]
    fn every_recording_word_is_recognised_by_the_os_name() {
        assert!(!RECORDING_WORDS.is_empty());
        for word in RECORDING_WORDS {
            let name = format!("{word}{RECORDING_TAIL}");
            assert_eq!(
                recognise_by_name(&name),
                capture(CaptureKind::Recording, CaptureBasis::OsName),
                "{name:?}"
            );
        }
    }

    #[test]
    fn a_german_screenshot_is_recognised() {
        assert!(STILL_WORDS.contains(&"Bildschirmfoto"));
        assert_eq!(
            recognise_by_name("Bildschirmfoto 2026-11-12 um 09.41.12.png"),
            capture(CaptureKind::Still, CaptureBasis::OsName)
        );
    }

    #[test]
    fn the_english_os_word_is_an_os_name_not_a_known_name() {
        assert_eq!(
            recognise_by_name("Screenshot 2026-11-12 at 09.41.12.png"),
            capture(CaptureKind::Still, CaptureBasis::OsName)
        );
    }

    #[test]
    fn cleanshot_is_a_known_name() {
        assert_eq!(
            recognise_by_name("CleanShot 2026-11-12 at 09.41.12.png"),
            capture(CaptureKind::Still, CaptureBasis::KnownName)
        );
    }

    #[test]
    fn the_old_english_rule_still_matches_without_a_date() {
        // Today's rule needs no digit; keeping it means nothing it grouped
        // falls out of the group.
        assert_eq!(
            recognise_by_name("screen shot of the bug.JPG"),
            capture(CaptureKind::Still, CaptureBasis::KnownName)
        );
        // ...but it never covered the formats the OS words added.
        assert_eq!(recognise_by_name("screenshot of the bug.gif"), None);
    }

    #[test]
    fn an_ordinary_image_is_not_a_capture() {
        assert_eq!(recognise_by_name("holiday.png"), None);
    }

    #[test]
    fn a_capture_word_on_a_text_file_is_not_a_capture() {
        assert_eq!(
            recognise_by_name("Screenshot 2026-11-12 at 09.41.12.txt"),
            None
        );
        assert_eq!(recognise_by_name("Bildschirmfoto 2026-11-12.txt"), None);
    }

    #[test]
    fn a_still_word_on_a_recording_extension_is_not_an_os_name() {
        assert_eq!(recognise_by_name("Bildschirmfoto 2026-11-12.mov"), None);
    }

    #[test]
    fn the_extension_is_compared_without_case() {
        assert_eq!(capture_kind_for_extension("PNG"), Some(CaptureKind::Still));
        assert_eq!(
            capture_kind_for_extension("Mov"),
            Some(CaptureKind::Recording)
        );
        assert_eq!(capture_kind_for_extension("webp"), None);
    }

    #[test]
    fn the_os_word_needs_a_digit_after_it() {
        assert_eq!(recognise_by_name("Bildschirmfoto ideas.png"), None);
    }

    #[test]
    fn the_weakest_basis_is_the_greatest() {
        assert!(CaptureBasis::OsAttribute < CaptureBasis::OsName);
        assert!(CaptureBasis::OsName < CaptureBasis::KnownName);
    }

    /// The committed table must hold every word the running OS uses. A new
    /// or changed word fails here, and `scripts/gen-capture-names.sh`
    /// regenerates the table.
    #[cfg(target_os = "macos")]
    #[test]
    fn capture_names_match_the_os() {
        const SOURCE: &str = "/System/Library/CoreServices/screencaptureui.app/Contents/Resources/Localizable.loctable";
        if !Path::new(SOURCE).exists() {
            println!(
                "capture_names_match_the_os: {SOURCE} is missing on this Mac; nothing to compare"
            );
            return;
        }
        let out = std::process::Command::new("/usr/bin/plutil")
            .args(["-convert", "json", "-o", "-", SOURCE])
            .output()
            .expect("run /usr/bin/plutil");
        assert!(out.status.success(), "plutil failed on {SOURCE}");
        let text = String::from_utf8(out.stdout).expect("plutil writes UTF-8");
        let root = json::parse(&text);
        let json::Value::Object(languages) = root else {
            panic!("the string table is not a JSON object");
        };

        let mut missing = Vec::new();
        let mut seen = 0;
        for (lang, table) in &languages {
            let json::Value::Object(table) = table else {
                continue;
            };
            for (key, words) in [
                ("Screenshot", STILL_WORDS),
                ("Screen Recording", RECORDING_WORDS),
            ] {
                let Some((_, json::Value::String(word))) = table.iter().find(|(k, _)| k == key)
                else {
                    continue;
                };
                seen += 1;
                if !words.contains(&word.as_str()) {
                    missing.push(format!("{lang} {key:?}: {word:?}"));
                }
            }
        }
        assert!(seen > 0, "no language in {SOURCE} has the capture keys");
        assert!(
            missing.is_empty(),
            "the OS has capture words the table lacks; run scripts/gen-capture-names.sh:\n{}",
            missing.join("\n")
        );
    }

    /// Just enough JSON to read plutil's output, because the crate takes no
    /// dependencies, not even for tests of this one table.
    #[cfg(target_os = "macos")]
    mod json {
        pub enum Value {
            Object(Vec<(String, Value)>),
            String(String),
            Other,
        }

        pub fn parse(text: &str) -> Value {
            let mut p = Parser {
                chars: text.chars().peekable(),
            };
            let value = p.value();
            p.space();
            assert!(p.chars.next().is_none(), "trailing text after JSON");
            value
        }

        struct Parser<'a> {
            chars: std::iter::Peekable<std::str::Chars<'a>>,
        }

        impl Parser<'_> {
            fn space(&mut self) {
                while self.chars.next_if(|c| c.is_ascii_whitespace()).is_some() {}
            }

            fn expect(&mut self, want: char) {
                self.space();
                assert_eq!(self.chars.next(), Some(want), "malformed JSON");
            }

            fn value(&mut self) -> Value {
                self.space();
                match self.chars.peek() {
                    Some('{') => self.object(),
                    Some('[') => {
                        self.chars.next();
                        self.space();
                        if self.chars.next_if_eq(&']').is_none() {
                            loop {
                                self.value();
                                self.space();
                                match self.chars.next() {
                                    Some(',') => continue,
                                    Some(']') => break,
                                    _ => panic!("malformed JSON array"),
                                }
                            }
                        }
                        Value::Other
                    }
                    Some('"') => Value::String(self.string()),
                    _ => {
                        // Numbers, true, false, null: read past them.
                        while self
                            .chars
                            .next_if(|c| !matches!(c, ',' | '}' | ']') && !c.is_ascii_whitespace())
                            .is_some()
                        {}
                        Value::Other
                    }
                }
            }

            fn object(&mut self) -> Value {
                self.expect('{');
                let mut fields = Vec::new();
                self.space();
                if self.chars.next_if_eq(&'}').is_some() {
                    return Value::Object(fields);
                }
                loop {
                    self.space();
                    let key = self.string();
                    self.expect(':');
                    fields.push((key, self.value()));
                    self.space();
                    match self.chars.next() {
                        Some(',') => continue,
                        Some('}') => return Value::Object(fields),
                        _ => panic!("malformed JSON object"),
                    }
                }
            }

            fn string(&mut self) -> String {
                assert_eq!(self.chars.next(), Some('"'), "expected a JSON string");
                let mut out = String::new();
                loop {
                    match self.chars.next().expect("unterminated JSON string") {
                        '"' => return out,
                        '\\' => match self.chars.next().expect("bad escape") {
                            'n' => out.push('\n'),
                            't' => out.push('\t'),
                            'r' => out.push('\r'),
                            'b' => out.push('\u{8}'),
                            'f' => out.push('\u{c}'),
                            'u' => {
                                let first = self.hex4();
                                let code = if (0xD800..0xDC00).contains(&first) {
                                    assert_eq!(self.chars.next(), Some('\\'));
                                    assert_eq!(self.chars.next(), Some('u'));
                                    let low = self.hex4();
                                    0x10000 + ((first - 0xD800) << 10) + (low - 0xDC00)
                                } else {
                                    first
                                };
                                out.push(char::from_u32(code).expect("valid code point"));
                            }
                            other => out.push(other),
                        },
                        c => out.push(c),
                    }
                }
            }

            fn hex4(&mut self) -> u32 {
                let digits: String = (0..4)
                    .map(|_| self.chars.next().expect("short \\u escape"))
                    .collect();
                u32::from_str_radix(&digits, 16).expect("hex \\u escape")
            }
        }
    }
}
