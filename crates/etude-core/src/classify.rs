//! Detectors.
//!
//! Design rule from docs/sweep/CRITIQUE.md § 7: no general clustering. Each detector
//! is individually explainable, high precision, low recall. A user must be able
//! to check the tool's reasoning by eye.
//!
//! Order matters. `sensitive` runs first and removes files from consideration by
//! every other detector — a tax document is not a shared-token candidate even
//! when forty files share its token.

use crate::scan::Entry;
use crate::Category;

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
const CREDENTIAL_NAMES: &[&str] =
    &["id_rsa", "id_ed25519", "id_dsa", "id.rsa", "credentials", "recovery_codes", ".env"];

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
    // Longest marker wins, so "tax_return" beats a stray substring match.
    SENSITIVE_MARKERS
        .iter()
        .filter(|(m, _)| lower.contains(m))
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
    camera_stem && matches!(e.ext.as_str(), "jpg" | "jpeg" | "heic" | "png" | "raw" | "dng" | "mov")
}

pub fn is_installer(e: &Entry) -> bool {
    matches!(e.ext.as_str(), "dmg" | "pkg" | "msi" | "deb" | "rpm" | "appimage")
}

/// Split a filename into lowercase tokens usable as group names.
///
/// Tokens are what the *user* already wrote. The naming rule in docs/sweep/SPEC.md
/// depends on this: sweep may only name a group with a string that already
/// exists in the filenames it contains.
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
    "final", "draft", "copy", "new", "old", "untitled", "document", "file", "version", "temp",
    "tmp", "backup", "export", "download", "downloads", "desktop", "screen", "shot", "image",
    "photo", "scan", "pdf", "doc", "docx", "png", "jpg", "the", "and", "for", "with",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(name: &str) -> Entry {
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
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
        assert_eq!(sensitive(&entry("recovery_codes.txt")), Some(Category::Credential));
    }

    #[test]
    fn screenshots_and_cameras_are_distinguished() {
        assert!(is_screenshot(&entry("Screenshot 2026-07-12 at 9.14.22 AM.png")));
        assert!(!is_screenshot(&entry("IMG_4471.HEIC")));
        assert!(is_camera(&entry("IMG_4471.HEIC")));
        assert!(!is_camera(&entry("Screenshot 2026-07-12 at 9.14.22 AM.png")));
    }

    #[test]
    fn tokens_drop_generic_words_so_groups_are_never_named_untitled() {
        let t = tokens("final_FINAL_v2.docx");
        assert!(!t.contains(&"final".to_string()), "generic token survived: {t:?}");
        let t = tokens("acme_logo_v3.psd");
        assert!(t.contains(&"acme".to_string()), "distinctive token lost: {t:?}");
    }
}
