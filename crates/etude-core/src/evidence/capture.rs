//! Screen-capture recognition, and how sure sweep is about it.
//!
//! `recognise` checks three kinds of evidence, strongest first, and the first
//! that matches decides:
//!
//! 1. `OsAttribute`: macOS marked the file with the attribute
//!    `com.apple.metadata:kMDItemIsScreenCapture`. Stills only.
//! 2. `OsName`: the name starts with the exact text macOS writes before the
//!    date, in one of the languages it ships, then a digit.
//! 3. `KnownName`: the English rule sweep has always used.
//!
//! The attribute does not depend on language, and it is stored on the file,
//! so a rename keeps it. A copy by a tool that drops attributes loses it,
//! which is what the names cover. The known names keep CleanShot and older
//! Macs, which wrote "Screen Shot", working.
//!
//! Sources (CONTRIBUTING.md requires them for platform claims):
//!
//! - The attribute on stills: observed on a GitHub `macos-15` runner
//!   (macOS 15.8.2, 24H232). `screencapture -x` wrote
//!   `com.apple.metadata:kMDItemIsScreenCapture` as a binary plist `true`,
//!   beside `kMDItemScreenCaptureType` = `display`. Primary, observed:
//!   <https://github.com/risingmoonscholar/etudes/actions/runs/43233341758>.
//! - No attribute on recordings: in the same run, a `screencapture -v -V 2`
//!   recording had no extended attributes at all. Recordings saved with
//!   Cmd-Shift-5 were not tested. So the attribute is asked of stills only,
//!   and recordings are known by name. `kMDItemIsScreenRecording` is never
//!   asked: nobody has seen it on a file.
//! - The text macOS starts a capture file name with, in every language it
//!   ships: `/System/Library/CoreServices/screencaptureui.app/Contents/
//!   Resources/Localizable.loctable`, keys `Screenshot` and
//!   `Screen Recording` (the words), `%@ %@ at %@` (the name format) and
//!   `yyyy-MM-dd` (the date format), read directly with `plutil` on macOS
//!   27.0.1 (26A434). Primary. The table is generated from it by
//!   `scripts/gen-capture-names.sh`; see `capture_names.rs`. The name format
//!   is read from the table only; it has not yet been checked against a real
//!   captured file.
//! - The output formats: `man screencapture` lists `-t` with `png` (the
//!   default), `pdf`, `jpg`, `tiff` "and other formats", and `-v` for video.
//!   Primary. `heic` comes from the English rule sweep already shipped.
//!
//! Nothing here touches the filesystem. The attribute question is asked
//! through the probe the caller passes, so this module reads no attributes
//! and no contents itself; the scan owns the one probe that does
//! (`scan::has_screen_capture_mark`).

use super::capture_names::{RECORDING_PREFIXES, STILL_PREFIXES};
use crate::scan::Entry;
use std::ffi::CStr;
use std::path::Path;

/// The attribute macOS writes on a screenshot it saves. Only its presence is
/// ever asked; its value is never read.
pub const ATTRIBUTE: &CStr = c"com.apple.metadata:kMDItemIsScreenCapture";

/// What a screen capture holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureKind {
    /// A still screenshot.
    Still,
    /// A screen recording.
    Recording,
}

impl CaptureKind {
    /// The kind's name in `--json` output and the capability declaration.
    pub fn key(self) -> &'static str {
        match self {
            CaptureKind::Still => "still",
            CaptureKind::Recording => "recording",
        }
    }
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
    /// The name starts the way macOS itself writes it, in some language.
    OsName,
    /// The name matches the English rule sweep used before it knew the OS
    /// words. It also covers CleanShot, which is not part of macOS.
    KnownName,
}

impl CaptureBasis {
    /// The basis's name in `--json` output and the capability declaration.
    pub fn key(self) -> &'static str {
        match self {
            CaptureBasis::OsAttribute => "os_attribute",
            CaptureBasis::OsName => "os_name",
            CaptureBasis::KnownName => "known_name",
        }
    }
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

/// Recognise a screen capture, strongest evidence first.
///
/// `probe` answers whether macOS marked the file at this path with
/// `ATTRIBUTE`. It is called at most once, and only for a file with a still
/// extension: never for a folder, a recording or any other extension. The
/// caller decides what a failed probe means; the scan's probe treats it as
/// "absent", so the file is judged by its name.
pub fn recognise(entry: &Entry, probe: impl FnOnce(&Path) -> bool) -> Option<Capture> {
    if entry.is_dir {
        return None;
    }
    if capture_kind_for_extension(&entry.ext) == Some(CaptureKind::Still) && probe(&entry.path) {
        return Some(Capture {
            kind: CaptureKind::Still,
            basis: CaptureBasis::OsAttribute,
        });
    }
    recognise_by_name(&entry.name)
}

/// Recognise a screen capture from its file name alone.
///
/// The OS prefixes are compared as exact code points and exact case, the
/// way macOS writes them. Nothing is stripped or normalised: the Dutch word
/// carries a soft hyphen (U+00AD), the French word a typographic apostrophe
/// (U+2019), Russian a no-break space (U+00A0) and Arabic right-to-left
/// marks (U+200F), and a name typed without them was not written by macOS.
///
/// The table is compiled in, so this matches the same names on every
/// platform.
pub fn recognise_by_name(file_name: &str) -> Option<Capture> {
    // The same split the scan uses, so `.png` alone has no extension here
    // either.
    let path = Path::new(file_name);
    let ext = path.extension()?.to_str()?;
    let kind = capture_kind_for_extension(ext)?;
    let stem = path.file_stem()?.to_str()?;

    let prefixes = match kind {
        CaptureKind::Still => STILL_PREFIXES,
        CaptureKind::Recording => RECORDING_PREFIXES,
    };
    if prefixes.iter().any(|prefix| starts_os_name(stem, prefix)) {
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

/// macOS names a capture as the prefix, then the date. The prefix already
/// holds everything the OS writes before the date's first digit, so the next
/// character must be that digit. Requiring it keeps an ordinary title that
/// happens to start with the word, such as "Screenshot ideas.png", out.
fn starts_os_name(stem: &str, prefix: &str) -> bool {
    stem.strip_prefix(prefix)
        .and_then(|rest| rest.as_bytes().first())
        .is_some_and(u8::is_ascii_digit)
}

/// The English rule sweep shipped before it knew the OS words, reproduced
/// exactly so no file it grouped changes group: the name starts with
/// `screenshot`, `screen shot` or `cleanshot` in any ASCII case.
///
/// It keeps its own narrower extension list (png, jpg, jpeg, heic), not the
/// whole capture set. Widening it would change which files earlier builds
/// grouped, and that is not this rule's job; the OS prefixes cover the other
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

    const STILL_TAIL: &str = "2026-11-12 at 09.41.12.png";
    const RECORDING_TAIL: &str = "2026-11-12 at 09.41.12.mov";

    fn capture(kind: CaptureKind, basis: CaptureBasis) -> Option<Capture> {
        Some(Capture { kind, basis })
    }

    #[test]
    fn every_still_prefix_is_recognised_by_the_os_name() {
        assert!(!STILL_PREFIXES.is_empty());
        for prefix in STILL_PREFIXES {
            let name = format!("{prefix}{STILL_TAIL}");
            assert_eq!(
                recognise_by_name(&name),
                capture(CaptureKind::Still, CaptureBasis::OsName),
                "{name:?}"
            );
        }
    }

    #[test]
    fn every_recording_prefix_is_recognised_by_the_os_name() {
        assert!(!RECORDING_PREFIXES.is_empty());
        for prefix in RECORDING_PREFIXES {
            let name = format!("{prefix}{RECORDING_TAIL}");
            assert_eq!(
                recognise_by_name(&name),
                capture(CaptureKind::Recording, CaptureBasis::OsName),
                "{name:?}"
            );
        }
    }

    #[test]
    fn a_german_screenshot_is_recognised() {
        assert!(STILL_PREFIXES.contains(&"Bildschirmfoto "));
        assert_eq!(
            recognise_by_name("Bildschirmfoto 2026-11-12 um 09.41.12.png"),
            capture(CaptureKind::Still, CaptureBasis::OsName)
        );
    }

    #[test]
    fn a_russian_screenshot_is_recognised() {
        // Russian writes a no-break space and a dash between the word and the
        // date, so a bare-word match never saw these names.
        let word =
            "\u{421}\u{43d}\u{438}\u{43c}\u{43e}\u{43a} \u{44d}\u{43a}\u{440}\u{430}\u{43d}\u{430}";
        let prefix = STILL_PREFIXES
            .iter()
            .find(|p| p.starts_with(word))
            .expect("the table has the Russian still prefix");
        assert_eq!(&prefix[word.len()..], "\u{a0}\u{2014} ");
        let name = format!("{prefix}12.11.2026 \u{432}\u{a0}09.41.12.png");
        assert_eq!(
            recognise_by_name(&name),
            capture(CaptureKind::Still, CaptureBasis::OsName)
        );
        // The bare word with a plain space is not how macOS names it.
        assert_eq!(recognise_by_name(&format!("{word} 12.11.2026.png")), None);
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
        // ...but it never covered the formats the OS prefixes added.
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

    fn entry(name: &str, is_dir: bool) -> Entry {
        Entry {
            path: Path::new("/fixture").join(name),
            name: name.to_string(),
            ext: Path::new(name)
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default(),
            size: 1,
            modified: None,
            is_dir,
            is_package: is_dir,
            capture: None,
        }
    }

    /// Recognise `name` with a probe that says "marked", and report whether
    /// the probe was asked.
    fn with_marked_probe(name: &str, is_dir: bool) -> (Option<Capture>, bool) {
        let mut asked = false;
        let capture = recognise(&entry(name, is_dir), |path| {
            assert_eq!(path, Path::new("/fixture").join(name));
            asked = true;
            true
        });
        (capture, asked)
    }

    #[test]
    fn a_marked_still_with_an_unrelated_name_is_an_os_attribute_capture() {
        assert_eq!(
            with_marked_probe("notes.png", false),
            (capture(CaptureKind::Still, CaptureBasis::OsAttribute), true)
        );
        // The attribute is stronger than a name macOS wrote.
        assert_eq!(
            with_marked_probe("Bildschirmfoto 2026-11-12 um 09.41.12.pdf", false),
            (capture(CaptureKind::Still, CaptureBasis::OsAttribute), true)
        );
    }

    #[test]
    fn the_probe_is_asked_for_still_extensions_only() {
        // Any other extension is never a capture and is never probed.
        assert_eq!(with_marked_probe("notes.txt", false), (None, false));
        assert_eq!(with_marked_probe("notes", false), (None, false));
        // A folder is never probed, whatever it is called.
        assert_eq!(with_marked_probe("shots.png", true), (None, false));
        // Recordings are known by name only: nobody has seen the attribute
        // on a recording, so it is not asked.
        assert_eq!(with_marked_probe("clip.mov", false), (None, false));
        assert_eq!(
            with_marked_probe("Screen Recording 2026-11-12 at 09.41.12.mov", false),
            (capture(CaptureKind::Recording, CaptureBasis::OsName), false)
        );
    }

    #[test]
    fn an_unmarked_still_falls_back_to_its_name() {
        let unmarked = |name: &str| recognise(&entry(name, false), |_| false);
        assert_eq!(unmarked("notes.png"), None);
        assert_eq!(
            unmarked("Bildschirmfoto 2026-11-12 um 09.41.12.png"),
            capture(CaptureKind::Still, CaptureBasis::OsName)
        );
        assert_eq!(
            unmarked("CleanShot 2026-11-12 at 09.41.12.png"),
            capture(CaptureKind::Still, CaptureBasis::KnownName)
        );
    }

    #[test]
    fn the_dutch_soft_hyphen_and_the_french_apostrophe_are_kept() {
        assert_eq!(
            recognise_by_name("Scherm\u{ad}afbeelding 2026-11-12 om 09.41.12.png"),
            capture(CaptureKind::Still, CaptureBasis::OsName)
        );
        assert_eq!(
            recognise_by_name("Schermafbeelding 2026-11-12 om 09.41.12.png"),
            None
        );
        assert_eq!(
            recognise_by_name("Capture d\u{2019}\u{e9}cran 2026-11-12 \u{e0} 09.41.12.png"),
            capture(CaptureKind::Still, CaptureBasis::OsName)
        );
        assert_eq!(
            recognise_by_name("Capture d'\u{e9}cran 2026-11-12 \u{e0} 09.41.12.png"),
            None
        );
    }

    /// The committed table must hold every prefix the running OS writes. A
    /// new or changed word or format fails here, and
    /// `scripts/gen-capture-names.sh` regenerates the table. The prefixes are
    /// derived here the same way the script derives them.
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
            // "LocProvenance" is bookkeeping, not a language.
            let json::Value::Object(table) = table else {
                continue;
            };
            if lang == "LocProvenance" {
                continue;
            }
            let get = |key: &str| match table.iter().find(|(k, _)| k == key) {
                Some((_, json::Value::String(value))) if !value.is_empty() => value.as_str(),
                _ => panic!("language {lang} has no string for key {key:?}"),
            };
            let (before, between) = split_name_format(lang, get("%@ %@ at %@"));
            let lead = date_lead(lang, get("yyyy-MM-dd"));
            for (key, prefixes) in [
                ("Screenshot", STILL_PREFIXES),
                ("Screen Recording", RECORDING_PREFIXES),
            ] {
                let prefix = format!("{before}{}{between}{lead}", get(key));
                seen += 1;
                if !prefixes.contains(&prefix.as_str()) {
                    missing.push(format!("{lang} {key:?}: {prefix:?}"));
                }
            }
        }
        assert!(seen > 0, "no language in {SOURCE} has the capture keys");
        assert!(
            missing.is_empty(),
            "the OS has capture prefixes the table lacks; run scripts/gen-capture-names.sh:\n{}",
            missing.join("\n")
        );
    }

    /// The text before the word and between the word and the date in a name
    /// format of three arguments (word, date, time). `%@` and Thai's
    /// `%[tt]@` are the only placeholders handled; anything else, such as a
    /// positional `%1$@` that could reorder the arguments, fails rather than
    /// being guessed at.
    #[cfg(target_os = "macos")]
    fn split_name_format(lang: &str, format: &str) -> (String, String) {
        let mut parts = vec![String::new()];
        let mut rest = format;
        while let Some(ch) = rest.chars().next() {
            if let Some(after) = rest
                .strip_prefix("%@")
                .or_else(|| rest.strip_prefix("%[tt]@"))
            {
                parts.push(String::new());
                rest = after;
            } else if let Some(after) = rest.strip_prefix("%%") {
                parts.last_mut().unwrap().push('%');
                rest = after;
            } else if ch == '%' {
                panic!("language {lang}: cannot parse placeholder in {format:?}");
            } else {
                parts.last_mut().unwrap().push(ch);
                rest = &rest[ch.len_utf8()..];
            }
        }
        assert_eq!(
            parts.len(),
            4,
            "language {lang}: expected three placeholders in {format:?}"
        );
        (parts[0].clone(), parts[1].clone())
    }

    /// The text a date format writes before its first number.
    #[cfg(target_os = "macos")]
    fn date_lead<'a>(lang: &str, format: &'a str) -> &'a str {
        let at = format
            .find(['y', 'Y', 'M', 'd'])
            .unwrap_or_else(|| panic!("language {lang}: no date field in {format:?}"));
        let (lead, fields) = format.split_at(at);
        let field = fields.chars().next().unwrap();
        let run = fields.chars().take_while(|c| *c == field).count();
        assert!(
            !(field == 'M' && run > 2),
            "language {lang}: date {format:?} starts with a month name, not a number"
        );
        assert!(
            !lead.chars().any(|c| c.is_ascii_alphabetic() || c == '\''),
            "language {lang}: cannot parse the text before the date in {format:?}"
        );
        lead
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
