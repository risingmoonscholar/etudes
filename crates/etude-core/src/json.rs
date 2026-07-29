//! Minimal JSON emitter.
//!
//! `etude-core` has zero dependencies and that is a load-bearing claim, so
//! `serde_json` is not an option. This is a writer only — the tools emit JSON
//! and never parse it, so a full parser would be unused surface.
//!
//! Escaping is the part worth getting right: filenames legitimately contain
//! quotes, backslashes, tabs, newlines and control characters, and the fixture
//! tree contains a tab on purpose.

use std::fmt::Write as _;
use std::path::Path;

/// Escape a string into a JSON string literal, including the quotes.
pub fn str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            // Everything below 0x20 must be escaped or the output is invalid.
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A path as a JSON string. Lossy conversion, because a path that is not UTF-8
/// still has to appear in the output rather than vanish from the accounting.
pub fn path(p: &Path) -> String {
    str(&p.to_string_lossy())
}

/// `["a", "b"]` from anything iterable of already-encoded values.
pub fn arr(items: impl IntoIterator<Item = String>) -> String {
    let inner: Vec<String> = items.into_iter().collect();
    format!("[{}]", inner.join(","))
}

/// `{"k": v}` from already-encoded values. Key order is preserved, so output is
/// byte-stable across runs — a plan that reorders cannot be diffed.
pub fn obj(fields: &[(&str, String)]) -> String {
    let inner: Vec<String> = fields.iter().map(|(k, v)| format!("{}:{}", str(k), v)).collect();
    format!("{{{}}}", inner.join(","))
}

pub fn num(n: impl std::fmt::Display) -> String {
    n.to_string()
}

pub fn bool(b: bool) -> String {
    if b { "true".into() } else { "false".into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_characters_in_filenames_are_escaped() {
        // The fixture tree contains a tab in a filename on purpose. Emitting it
        // raw produces invalid JSON that an agent cannot parse.
        assert_eq!(str("weird\tname.txt"), r#""weird\tname.txt""#);
        assert_eq!(str("line\nbreak"), r#""line\nbreak""#);
        assert_eq!(str("quote\"inside"), r#""quote\"inside""#);
        assert_eq!(str("back\\slash"), r#""back\\slash""#);
        // The escaped form is the point: a raw 0x07 byte makes the JSON
        // invalid, and it is exactly what a hostile filename would carry.
        assert_eq!(str("bell\u{07}"), r#""bell\u0007""#);
    }

    #[test]
    fn unicode_passes_through_unescaped() {
        // Valid UTF-8 needs no escaping, and mangling it would break round trips
        // through the fixture tree's NFC/NFD pair.
        assert_eq!(str("café"), "\"café\"");
    }

    #[test]
    fn objects_keep_key_order_so_output_is_diffable() {
        let a = obj(&[("b", num(1)), ("a", num(2))]);
        assert_eq!(a, r#"{"b":1,"a":2}"#, "key order was not preserved");
    }

    #[test]
    fn empty_containers_are_valid() {
        assert_eq!(arr(Vec::new()), "[]");
        assert_eq!(obj(&[]), "{}");
    }
}
