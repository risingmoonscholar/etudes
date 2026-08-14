//! Detectors.
//!
//! Design rule: no general clustering. Each detector is individually
//! explainable, high precision, low recall. A user must be able to check the
//! tool's reasoning by eye.
//!
//! Order matters. `sensitive` runs first and removes files from consideration by
//! every other detector. A tax document is not a shared-token candidate even
//! when forty files share its token.

use crate::Category;
use crate::scan::Entry;

/// Filename markers that make sweep refuse to organise a file.
///
/// These are matched against the lowercased file name. Precision is favoured
/// over recall in one direction only: a false positive costs the user nothing
/// (the file is left alone), a false negative moves a sensitive document.
const SENSITIVE_MARKERS: &[(&str, Category)] = &[
    // Tax
    ("w2", Category::Tax),
    ("w-2", Category::Tax),
    ("1099", Category::Tax),
    ("1040", Category::Tax),
    ("tax_return", Category::Tax),
    ("tax return", Category::Tax),
    ("taxreturn", Category::Tax),
    ("irs", Category::Tax),
    ("p60", Category::Tax),
    ("p45", Category::Tax),
    // Identity
    ("ssn", Category::Identity),
    ("social_security", Category::Identity),
    ("passport", Category::Identity),
    ("drivers_license", Category::Identity),
    ("driving_licence", Category::Identity),
    ("birth_certificate", Category::Identity),
    ("national_insurance", Category::Identity),
    ("green_card", Category::Identity),
    ("visa_application", Category::Identity),
    // Medical
    ("mri", Category::Medical),
    ("lab_result", Category::Medical),
    ("lab results", Category::Medical),
    ("biopsy", Category::Medical),
    ("prescription", Category::Medical),
    ("diagnosis", Category::Medical),
    ("insurance_claim", Category::Medical),
    ("medical", Category::Medical),
    ("oncology", Category::Medical),
    ("radiology", Category::Medical),
    // Financial
    ("bank_statement", Category::Financial),
    ("bank statement", Category::Financial),
    ("account_statement", Category::Financial),
    ("payslip", Category::Financial),
    ("paystub", Category::Financial),
    ("mortgage", Category::Financial),
    ("brokerage", Category::Financial),
    // Legal
    ("divorce", Category::Legal),
    ("custody", Category::Legal),
    ("settlement", Category::Legal),
    ("subpoena", Category::Legal),
    ("will_and_testament", Category::Legal),
];

/// Extensions and exact names that are credentials regardless of context.
const CREDENTIAL_EXTS: &[&str] = &["pem", "key", "p12", "pfx", "keychain", "kdbx", "jks", "asc"];
const CREDENTIAL_NAMES: &[&str] = &[
    "id_rsa",
    "id_ed25519",
    "id_dsa",
    "id.rsa",
    "credentials",
    "recovery_codes",
    ".env",
];

/// Extensions DCF assigns to a camera's own output: the still-image and
/// thumbnail types the standard names, plus the video containers a phone
/// records to. A guard against a device-assigned counter is only correct on
/// files a device would actually produce.
const DCF_IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "heic", "heif", "thm", "tif", "tiff", "dng", "raw", "cr2", "nef", "arw", "mov",
    "mp4",
];

/// Whether `stem` has the shape DCF (JEITA CP-3461 v2.0) specifies for a
/// camera-assigned image name: four alphanumeric characters, then a number
/// in 0001..9999, no more and no fewer digits.
///
/// Source: <https://en.wikipedia.org/wiki/Design_rule_for_Camera_File_system>,
/// a secondary summary of CP-3461; the primary standard is paywalled and was
/// not read. Documented prefixes are "100_ DSC0 DSC_ DSCF IMG_ MOV_ P000".
/// This checks the general shape rather than a prefix list: Nikon's "DSCN"
/// and Google's "PXL_" both conform to the same four-then-four rule without
/// appearing in that list, and a vendor list is exactly the kind of platform
/// guess CONTRIBUTING.md now asks not to make.
fn is_dcf_camera_stem(stem: &str) -> bool {
    let chars: Vec<char> = stem.chars().collect();
    if chars.len() != 8 {
        return false;
    }
    let (prefix, digits) = chars.split_at(4);
    prefix
        .iter()
        .all(|c| c.is_ascii_alphanumeric() || *c == '_')
        && digits.iter().all(char::is_ascii_digit)
        && digits != ['0', '0', '0', '0']
}

/// Does this file look like a personal record? Returns the category if so.
pub fn sensitive(e: &Entry) -> Option<Category> {
    let lower = e.name.to_ascii_lowercase();

    if CREDENTIAL_EXTS.contains(&e.ext.as_str()) {
        return Some(Category::Credential);
    }
    let stem = lower.rsplit_once('.').map(|(s, _)| s).unwrap_or(&lower);
    if CREDENTIAL_NAMES.iter().any(|n| stem == *n || lower == *n) {
        return Some(Category::Credential);
    }

    // A camera's own counter carries no information about content, so a
    // numeric marker matching only because it landed inside that counter is
    // coincidence, not evidence. Word markers are unaffected: "passport" or
    // "tax_return" inside a DCF-shaped stem is not something a camera wrote.
    let camera_named = DCF_IMAGE_EXTS.contains(&e.ext.as_str()) && is_dcf_camera_stem(stem);

    // Longest marker wins, so "tax_return" beats a stray substring match.
    SENSITIVE_MARKERS
        .iter()
        .filter(|(m, _)| lower.contains(m))
        .filter(|(m, _)| !(camera_named && m.chars().all(|c| c.is_ascii_digit())))
        .max_by_key(|(m, _)| m.len())
        .map(|(_, c)| *c)
}

/// macOS and common cross-platform screenshot naming.
pub fn is_screenshot(e: &Entry) -> bool {
    let l = e.name.to_ascii_lowercase();
    (l.starts_with("screenshot") || l.starts_with("screen shot") || l.starts_with("cleanshot"))
        && matches!(e.ext.as_str(), "png" | "jpg" | "jpeg" | "heic")
}

/// Camera-style stems that indicate an unedited capture.
pub fn is_camera(e: &Entry) -> bool {
    let l = e.name.to_ascii_lowercase();
    let camera_stem = l.starts_with("img_")
        || l.starts_with("dsc")
        || l.starts_with("pxl_")
        || l.starts_with("dji_")
        || l.starts_with("gopro");
    camera_stem
        && matches!(
            e.ext.as_str(),
            "jpg" | "jpeg" | "heic" | "png" | "raw" | "dng" | "mov"
        )
}

pub fn is_installer(e: &Entry) -> bool {
    matches!(
        e.ext.as_str(),
        "dmg" | "pkg" | "msi" | "deb" | "rpm" | "appimage"
    )
}

/// Split a filename into lowercase tokens usable as group names.
///
/// Tokens are what the *user* already wrote. The naming rule depends on this:
/// sweep may only name a group with a string that already exists in the
/// filenames it contains.
pub fn tokens(name: &str) -> Vec<String> {
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    stem.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 3)
        .filter(|t| !t.chars().all(|c| c.is_ascii_digit()))
        .filter(|t| !STOP_TOKENS.contains(&t.to_ascii_lowercase().as_str()))
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

/// Tokens too generic to name a group after.
const STOP_TOKENS: &[&str] = &[
    "final",
    "draft",
    "copy",
    "new",
    "old",
    "untitled",
    "document",
    "file",
    "version",
    "temp",
    "tmp",
    "backup",
    "export",
    "download",
    "downloads",
    "desktop",
    "screen",
    "shot",
    "image",
    "photo",
    "scan",
    "pdf",
    "doc",
    "docx",
    "png",
    "jpg",
    "the",
    "and",
    "for",
    "with",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(name: &str) -> Entry {
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        Entry {
            path: PathBuf::from(name),
            name: name.to_string(),
            ext,
            size: 1,
            modified: None,
            is_dir: false,
            is_package: false,
        }
    }

    #[test]
    fn every_synthetic_sensitive_fixture_is_caught() {
        // This is the acceptance test that makes the core promise falsifiable.
        for name in fixtures::SENSITIVE_NAMES {
            assert!(
                sensitive(&entry(name)).is_some(),
                "sensitive detector missed a fixture: {name}"
            );
        }
    }

    #[test]
    fn ordinary_files_are_not_flagged_sensitive() {
        for name in fixtures::ORDINARY_NAMES {
            assert!(
                sensitive(&entry(name)).is_none(),
                "false positive on an ordinary file: {name}"
            );
        }
    }

    #[test]
    fn credentials_are_caught_by_extension_and_by_name() {
        assert_eq!(sensitive(&entry("server.pem")), Some(Category::Credential));
        assert_eq!(sensitive(&entry("id_rsa")), Some(Category::Credential));
        assert_eq!(
            sensitive(&entry("recovery_codes.txt")),
            Some(Category::Credential)
        );
    }

    #[test]
    fn screenshots_and_cameras_are_distinguished() {
        assert!(is_screenshot(&entry(
            "Screenshot 2026-07-12 at 9.14.22 AM.png"
        )));
        assert!(!is_screenshot(&entry("IMG_4471.HEIC")));
        assert!(is_camera(&entry("IMG_4471.HEIC")));
        assert!(!is_camera(&entry(
            "Screenshot 2026-07-12 at 9.14.22 AM.png"
        )));
    }

    #[test]
    fn tokens_drop_generic_words_so_groups_are_never_named_untitled() {
        let t = tokens("final_FINAL_v2.docx");
        assert!(
            !t.contains(&"final".to_string()),
            "generic token survived: {t:?}"
        );
        let t = tokens("acme_logo_v3.psd");
        assert!(
            t.contains(&"acme".to_string()),
            "distinctive token lost: {t:?}"
        );
    }

    /// Files a device actually produces. Shapes drawn from the DCF standard
    /// (JEITA CP-3461 v2.0) and from Apple's documented iPhone naming, cited
    /// in `is_dcf_camera_stem`'s doc comment. Not invented: a corpus built
    /// from guesses about a platform proves nothing about that platform,
    /// which is the mistake this fix exists to correct.
    ///
    /// DCF numbers are exactly four digits, so a four-digit marker can only
    /// collide by equality: IMG_1040 and IMG_1099 are the only two possible
    /// hits in a full 0001..9999 folder, which is what makes these two the
    /// meaningful cases rather than an arbitrary pair.
    #[test]
    fn device_named_files_are_never_flagged_by_a_counter_collision() {
        let camera_named = [
            "IMG_1040.HEIC", // the collision this issue was filed over
            "IMG_1099.jpg",
            "DSC_1040.JPG", // documented DCF prefix
            "DSCN1040.JPG", // Nikon; conforms to the shape, not the prefix list
            "PXL_1040.jpg", // Google Pixel; same
            "100_1040.jpg", // documented DCF prefix
            "MOV_1040.mp4", // DCF allows non-JPG extensions for video
        ];
        for name in camera_named {
            assert_eq!(
                sensitive(&entry(name)),
                None,
                "{name} conforms to DCF and must not be flagged"
            );
        }
    }

    /// Real sensitive filenames, taken from this repository's own fixture
    /// generator (crates/fixtures/src/bin/mkfx.rs) rather than invented, plus
    /// the macOS Finder duplicate suffix and the one adversarial case the fix
    /// knowingly does not cover.
    #[test]
    fn real_sensitive_files_are_still_refused() {
        let must_refuse = [
            "1099-INT_first_national.pdf", // mkfx
            "2024-1099-INT.pdf",           // mkfx
            "W2_2024.pdf",                 // mkfx
            "tax_return_2023_filed.pdf",   // mkfx
            "2024-1099-INT copy.pdf",      // Finder duplicate suffix
            "IMG_1040.pdf",                // camera-shaped stem, document extension
            "scan_1099_int.jpg",           // human-named, not device-shaped
        ];
        for name in must_refuse {
            assert!(
                sensitive(&entry(name)).is_some(),
                "{name} must still be refused"
            );
        }
    }

    /// The residual risk, named rather than hidden. A file that happens to be
    /// exactly DCF-shaped AND carries a numeric marker is not distinguishable
    /// from a camera file by shape alone, and stops being refused. Documented
    /// in the fix's commit and in this test so the gap is asserted, not
    /// silently accepted.
    #[test]
    fn a_dcf_shaped_name_with_a_numeric_marker_is_the_known_gap() {
        assert_eq!(
            sensitive(&entry("TAX_1040.jpg")),
            None,
            "known gap: a DCF-shaped stem is indistinguishable from a camera's \
             own counter. If this ever starts refusing, update the comment in \
             is_dcf_camera_stem rather than leaving it describing a gap that \
             closed by accident."
        );
    }

    /// Word markers are untouched by the guard. It only ever removes a
    /// NUMERIC marker on a camera-shaped name; "passport" or "medical" inside
    /// one is not something a camera would write and stays refused.
    #[test]
    fn word_markers_are_not_affected_by_the_camera_guard() {
        assert!(sensitive(&entry("passport_1040.jpg")).is_some());
    }
}
