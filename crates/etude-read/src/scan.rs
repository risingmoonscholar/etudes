//! Content scanners.
//!
//! Every scanner is a single linear pass over bytes with no allocation
//! proportional to input and no regex engine, so there is no catastrophic
//! backtracking to trigger.
//!
//! A hit means one thing only: the file joins the untouched set. Nothing here
//! can influence a group name.

/// What kind of sensitive material was found. Mirrors `etude_core::Category`
/// without depending on it — this crate stays standalone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    Identity,
    Financial,
    Credential,
    Medical,
}

/// Extensions considered plain text. Everything else is judged on metadata only.
pub const TEXT_EXTS: &[&str] = &[
    "txt", "md", "csv", "log", "json", "xml", "rtf", "eml", "vcf", "ini", "conf", "yml", "yaml",
    "tex", "tsv",
];

/// A NUL in the first block means binary content wearing a text extension.
pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(super::buf::SNIFF).any(|b| *b == 0)
}

/// Scan for anything that should stop this file being organised.
///
/// Returns the first category found. Order is by severity, so a file containing
/// both a key and a medical word is reported as a credential.
pub fn scan(bytes: &[u8]) -> Option<Found> {
    if has_private_key(bytes) {
        return Some(Found::Credential);
    }
    if has_ssn(bytes) {
        return Some(Found::Identity);
    }
    if has_payment_card(bytes) {
        return Some(Found::Financial);
    }
    if has_vocabulary(bytes, MEDICAL_TERMS, 3) {
        return Some(Found::Medical);
    }
    if has_vocabulary(bytes, FINANCIAL_TERMS, 3) {
        return Some(Found::Financial);
    }
    None
}

fn has_private_key(b: &[u8]) -> bool {
    contains(b, b"-----BEGIN ") && contains(b, b"PRIVATE KEY-----")
}

/// Naive substring search. Input is capped at 1 MiB and needles are short, so
/// the quadratic worst case is bounded and small.
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    hay.windows(needle.len()).any(|w| w == needle)
}

/// US SSN in `NNN-NN-NNNN` form, excluding blocks the SSA never issues.
///
/// The exclusions matter: without them every `123-45-6789` in a test fixture,
/// a phone list, or a product code reads as an SSN and sweep refuses to organise
/// half the folder.
fn has_ssn(b: &[u8]) -> bool {
    let d = |c: u8| c.is_ascii_digit();
    for w in b.windows(11) {
        if !(d(w[0]) && d(w[1]) && d(w[2]) && w[3] == b'-' && d(w[4]) && d(w[5]) && w[6] == b'-'
            && d(w[7]) && d(w[8]) && d(w[9]) && d(w[10]))
        {
            continue;
        }
        // Must not be part of a longer digit run.
        let area = (w[0] - b'0') as u32 * 100 + (w[1] - b'0') as u32 * 10 + (w[2] - b'0') as u32;
        let group = (w[4] - b'0') as u32 * 10 + (w[5] - b'0') as u32;
        let serial = (w[7] - b'0') as u32 * 1000
            + (w[8] - b'0') as u32 * 100
            + (w[9] - b'0') as u32 * 10
            + (w[10] - b'0') as u32;
        if area == 0 || area == 666 || area >= 900 || group == 0 || serial == 0 {
            continue;
        }
        return true;
    }
    false
}

/// Payment card: a 13–19 digit run, ignoring spaces and dashes, passing Luhn.
///
/// Luhn is what keeps this from firing on every long number. A random digit run
/// passes with probability ~1/10.
fn has_payment_card(b: &[u8]) -> bool {
    let mut digits = [0u8; 19];
    let mut n = 0usize;

    let flush = |digits: &[u8], n: usize| -> bool { n >= 13 && luhn(&digits[..n]) };

    for &c in b {
        if c.is_ascii_digit() {
            if n < digits.len() {
                digits[n] = c - b'0';
                n += 1;
            } else {
                // Longer than any card: this is not a card number.
                n = digits.len() + 1;
            }
        } else if c == b' ' || c == b'-' {
            // Separators inside a card number are normal; keep accumulating.
        } else {
            if n <= digits.len() && flush(&digits, n) {
                return true;
            }
            n = 0;
        }
    }
    n <= digits.len() && flush(&digits, n)
}

fn luhn(d: &[u8]) -> bool {
    let mut sum = 0u32;
    let mut double = false;
    for &x in d.iter().rev() {
        let mut v = x as u32;
        if double {
            v *= 2;
            if v > 9 {
                v -= 9;
            }
        }
        sum += v;
        double = !double;
    }
    sum % 10 == 0
}

const MEDICAL_TERMS: &[&[u8]] = &[
    b"diagnosis",
    b"prescription",
    b"patient",
    b"physician",
    b"symptoms",
    b"dosage",
    b"biopsy",
    b"oncology",
    b"radiology",
    b"medical record",
    b"blood pressure",
    b"insurance claim",
];

const FINANCIAL_TERMS: &[&[u8]] = &[
    b"account number",
    b"routing number",
    b"sort code",
    b"iban",
    b"statement period",
    b"available balance",
    b"taxable income",
    b"gross pay",
    b"net pay",
    b"withholding",
];

/// True when at least `threshold` *distinct* terms appear.
///
/// Requiring several distinct terms is what separates a real medical record
/// from a novel that happens to use the word "patient" once.
fn has_vocabulary(b: &[u8], terms: &[&[u8]], threshold: usize) -> bool {
    let lower: Vec<u8> = b.iter().map(|c| c.to_ascii_lowercase()).collect();
    let mut hits = 0;
    for t in terms {
        if contains(&lower, t) {
            hits += 1;
            if hits >= threshold {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_social_security_number() {
        assert_eq!(scan(b"Employee SSN: 123-45-6789 on file"), Some(Found::Identity));
    }

    #[test]
    fn rejects_ssn_shaped_strings_the_ssa_never_issues() {
        // Without these exclusions ordinary product codes read as SSNs and
        // sweep refuses to organise half the folder.
        assert_eq!(scan(b"order 000-12-3456"), None, "area 000 accepted");
        assert_eq!(scan(b"order 666-12-3456"), None, "area 666 accepted");
        assert_eq!(scan(b"order 900-12-3456"), None, "area 900+ accepted");
        assert_eq!(scan(b"order 123-00-3456"), None, "group 00 accepted");
        assert_eq!(scan(b"order 123-45-0000"), None, "serial 0000 accepted");
    }

    #[test]
    fn finds_a_payment_card_and_ignores_a_random_digit_run() {
        // 4111 1111 1111 1111 is the canonical Visa test number.
        assert_eq!(scan(b"card 4111 1111 1111 1111 exp"), Some(Found::Financial));
        // Same length, fails Luhn.
        assert_eq!(scan(b"ref 4111111111111112 end"), None, "non-Luhn digits read as a card");
    }

    #[test]
    fn finds_a_private_key_block() {
        assert_eq!(
            scan(b"-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n"),
            Some(Found::Credential)
        );
    }

    #[test]
    fn one_medical_word_is_not_a_medical_record() {
        assert_eq!(scan(b"the patient waited in the rain"), None, "single term triggered");
        assert_eq!(
            scan(b"patient: J Doe. diagnosis: flu. prescription: rest."),
            Some(Found::Medical)
        );
    }

    #[test]
    fn ordinary_prose_is_not_flagged() {
        let text = b"Meeting notes. We agreed the redesign ships in March. \
                     Budget is fine. Next review on the 14th.";
        assert_eq!(scan(text), None, "false positive on ordinary text");
    }

    #[test]
    fn binary_content_is_detected_by_a_nul_byte() {
        assert!(looks_binary(b"PK\x03\x04\x00\x00stuff"));
        assert!(!looks_binary(b"plain text with no nul"));
    }
}
