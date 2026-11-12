#!/usr/bin/env bash
# Write crates/etude-core/src/evidence/capture_names.rs from the text macOS
# itself puts at the start of a screen capture's file name.
#
# A typed list of localized words is a claim; this script makes it an
# observation. It reads screencaptureui's own string table and, for every
# language, rebuilds the start of the file name the OS writes: the format
# text around the word, the word for a still screenshot or a screen
# recording, and whatever the date format writes before its first digit.
# It writes those prefixes out as two sorted Rust slices. A bare word is not
# enough: Russian puts a no-break space and a dash after it, Arabic a
# right-to-left mark before it, and those names never matched. Apple has changed the English word once
# already ("Screen Shot" became "Screenshot"), and the Dutch word carries a
# soft hyphen nobody would type, so the list is never written by hand.
#
# Any format the script cannot parse stops it with an error. It never guesses.
#
# Builds never run this. etude-core has zero dependencies and builds on Linux,
# so the output is committed and the macOS-only test `capture_names_match_the_os`
# catches the day the OS and the committed table disagree.
#
# Every non-ASCII character is written as a \u{..} escape, so an invisible
# character (U+00AD in Dutch) shows up in review instead of hiding in plain
# sight. Running the script twice on the same OS gives the same file.
#
# Usage:  scripts/gen-capture-names.sh
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

SOURCE=/System/Library/CoreServices/screencaptureui.app/Contents/Resources/Localizable.loctable
OUT=crates/etude-core/src/evidence/capture_names.rs

if [ ! -f "$SOURCE" ]; then
    echo "gen-capture-names: $SOURCE not found; this script needs macOS" >&2
    exit 1
fi

VERSION=$(sw_vers -productVersion)
BUILD=$(sw_vers -buildVersion)

mkdir -p "$(dirname "$OUT")"
/usr/bin/plutil -convert json -o - "$SOURCE" |
    python3 -c '
import json
import sys

source, version, build = sys.argv[1], sys.argv[2], sys.argv[3]

# The keys are the English source strings, so they read the same in every
# language table. They were picked by reading the English table: the two
# words that start a saved capture, the format that builds the file name
# from (word, date, time), and the date format inside it.
STILL_KEY = "Screenshot"
RECORDING_KEY = "Screen Recording"
NAME_FORMAT_KEY = "%@ %@ at %@"
DATE_FORMAT_KEY = "yyyy-MM-dd"

table = json.load(sys.stdin)
# "LocProvenance" is bookkeeping, not a language.
languages = sorted(k for k, v in table.items() if k != "LocProvenance" and isinstance(v, dict))


def fail(message):
    sys.exit("gen-capture-names: " + message)


def text(lang, key):
    value = table[lang].get(key)
    if not isinstance(value, str) or not value:
        fail(f"language {lang} has no string for key {key!r}")
    return value


def split_format(lang, fmt):
    """The literal text around each placeholder: [before, between1, between2, after].

    Only two placeholder shapes occur, and both are handled by name: %@, and
    Thai %[tt]@, which is still a plain object argument. Anything else, such
    as a positional %1$@ that could reorder the arguments, stops the script
    rather than being guessed at.
    """
    parts, current, i = [], "", 0
    while i < len(fmt):
        ch = fmt[i]
        if ch != "%":
            current += ch
            i += 1
        elif fmt.startswith("%@", i):
            parts.append(current)
            current, i = "", i + 2
        elif fmt.startswith("%[tt]@", i):
            parts.append(current)
            current, i = "", i + 6
        elif fmt.startswith("%%", i):
            current += "%"
            i += 2
        else:
            fail(f"language {lang}: cannot parse placeholder at {fmt[i:]!r} in {fmt!r}")
    parts.append(current)
    if len(parts) != 4:
        fail(f"language {lang}: expected three placeholders in {fmt!r}")
    return parts


def date_lead(lang, fmt):
    """The characters a date format writes before its first number."""
    for i, ch in enumerate(fmt):
        if ch in "yYMd":
            lead, field = fmt[:i], ch
            run = len(fmt[i:]) - len(fmt[i:].lstrip(field))
            if field == "M" and run > 2:
                fail(f"language {lang}: date {fmt!r} starts with a month name, not a number")
            if any(c.isascii() and c.isalpha() for c in lead) or "\x27" in lead:
                fail(f"language {lang}: cannot parse the text before the date in {fmt!r}")
            return lead
    fail(f"language {lang}: no date field in {fmt!r}")


def prefixes(key):
    found = set()
    for lang in languages:
        before, between, _, _ = split_format(lang, text(lang, NAME_FORMAT_KEY))
        lead = date_lead(lang, text(lang, DATE_FORMAT_KEY))
        found.add(before + text(lang, key) + between + lead)
    # Python sorts by code point, which is the order Rust uses for &str.
    return sorted(found)


def rust(word):
    out = []
    for ch in word:
        if " " <= ch <= "~" and ch not in "\"\\":
            out.append(ch)
        else:
            out.append("\\u{%x}" % ord(ch))
    return "\"" + "".join(out) + "\""


def slice_lines(name, items):
    lines = ["#[rustfmt::skip]", f"pub const {name}: &[&str] = &["]
    lines += [f"    {rust(w)}," for w in items]
    lines.append("];")
    return lines


still = prefixes(STILL_KEY)
recording = prefixes(RECORDING_KEY)

lines = [
    "//! The exact text macOS puts at the start of a screen capture file name,",
    "//! before the first digit of the date, in every language the OS ships.",
    "//! Sorted, without duplicates.",
    "",
    "// @generated by scripts/gen-capture-names.sh. Do not edit by hand.",
    "//",
    f"// Source: {source}",
    f"// Read on macOS {version} ({build}) with `plutil -convert json`.",
    f"// Languages: {len(languages)}.",
    "//",
    "// Each prefix is built per language from four keys:",
    f"//   name format {json.dumps(NAME_FORMAT_KEY)}, read as (word, date, time);",
    f"//   the word, key {json.dumps(STILL_KEY)} (still) or {json.dumps(RECORDING_KEY)} (recording);",
    f"//   date format {json.dumps(DATE_FORMAT_KEY)}.",
    "// Prefix = the format text before the word, the word, the format text",
    "// between the word and the date, and any text the date format writes",
    "// before its first number (Arabic writes U+200F there).",
    "//",
    "// The name format key is read from the string table only. In this change",
    "// it has not been checked against a real captured file.",
    "//",
    "// Non-ASCII characters are written as escapes so invisible ones, such as",
    "// the soft hyphen (U+00AD) in the Dutch word, can be seen in review.",
    "",
    "/// Text that starts the name of a saved screenshot, up to the date.",
]
lines += slice_lines("STILL_PREFIXES", still)
lines += ["", "/// Text that starts the name of a saved screen recording, up to the date."]
lines += slice_lines("RECORDING_PREFIXES", recording)
sys.stdout.write("\n".join(lines) + "\n")
' "$SOURCE" "$VERSION" "$BUILD" >"$OUT.tmp"

mv "$OUT.tmp" "$OUT"
echo "wrote $OUT"
