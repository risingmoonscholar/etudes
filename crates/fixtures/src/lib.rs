//! Synthetic adversarial fixture tree.
//!
//! No real user file is read during development or testing. Everything sweep is
//! tested against is generated here, including decoy "sensitive" documents whose
//! contents are obviously fake but whose *names* look exactly like the real
//! thing. v0.1 classifies on names. Names are what must be adversarial.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Every filename the generator creates, so tests can assert that none of them
/// appear in `--quiet` output or in any file sweep writes.
pub const SENSITIVE_NAMES: &[&str] = &[
    "W2_2024_acme_corp.pdf",
    "1099-INT_first_national.pdf",
    "tax_return_2023_filed.pdf",
    "SSN_card_scan.jpg",
    "passport_photo_page.png",
    "drivers_license_front.heic",
    "MRI_results_2026-03.pdf",
    "lab_results_cbc.pdf",
    "insurance_claim_denial.pdf",
    "bank_statement_march.pdf",
    "id.rsa",
    "credentials.csv",
    "recovery_codes.txt",
    "divorce_settlement_draft.docx",
];

pub const ORDINARY_NAMES: &[&str] = &[
    "acme_logo_v3.psd",
    "acme_homepage_mock.fig",
    "acme_brand_guide.pdf",
    "acme_palette.sketch",
    "acme_nav_spec.md",
    "bali_beach_01.jpg",
    "bali_beach_02.jpg",
    "bali_temple.jpg",
    "notes.txt",
    "Untitled 3.pdf",
    "final_FINAL_v2.docx",
];

/// A generated tree plus the facts a test needs to make assertions about it.
#[derive(Debug)]
pub struct Fixture {
    pub root: PathBuf,
    /// Absolute paths of files that must never be moved.
    pub sensitive: Vec<PathBuf>,
    /// Absolute path of the package directory that must stay opaque.
    pub package_dir: PathBuf,
    /// Symlink pointing outside the root. Must never be followed.
    pub escaping_symlink: PathBuf,
}

/// Build the full adversarial tree under `root`.
///
/// `root` must not exist or must be empty. The generator writes inside `root`
/// except for `sweep_fixture_outside/secret_outside.txt` beside it, the target
/// required by the escaping-symlink fixture.
pub fn build(root: &Path) -> io::Result<Fixture> {
    fs::create_dir_all(root)?;

    let mut sensitive = Vec::new();

    // --- Decoy sensitive documents -----------------------------------------
    // Contents are obviously fake. The *names* are the adversarial part.
    for name in SENSITIVE_NAMES {
        let p = root.join(name);
        fs::write(&p, b"SYNTHETIC TEST FIXTURE - NOT REAL DATA\n")?;
        sensitive.push(p);
    }

    // --- Ordinary files that should group ----------------------------------
    for name in ORDINARY_NAMES {
        fs::write(root.join(name), b"synthetic\n")?;
    }

    // --- Screenshots: the highest-precision detector ------------------------
    for i in 1..=34 {
        let name = format!(
            "Screenshot 2026-07-{:02} at 9.{:02}.11 AM.png",
            (i % 28) + 1,
            i
        );
        fs::write(root.join(name), b"synthetic\n")?;
    }

    // --- Camera burst -------------------------------------------------------
    for i in 4400..4427 {
        fs::write(root.join(format!("IMG_{i}.HEIC")), b"synthetic\n")?;
    }

    // --- Installers ---------------------------------------------------------
    for name in ["Docker-4.28.0.dmg", "node-v22.1.0.pkg", "Figma-124.5.dmg"] {
        fs::write(root.join(name), b"synthetic\n")?;
    }

    // --- Innocent names, sensitive contents ---------------------------------
    // These are the v0.3 case: metadata alone cannot refuse them, so only
    // content inspection can. Names are deliberately boring.
    fs::write(
        root.join("notes-a.txt"),
        b"call mom\nSSN 123-45-6789\npick up milk\n",
    )?;
    fs::write(
        root.join("notes-b.txt"),
        b"card 4111 1111 1111 1111 exp 03/29\n",
    )?;
    fs::write(
        root.join("notes-c.md"),
        b"patient: J Doe\ndiagnosis: flu\nprescription: rest\n",
    )?;
    fs::write(
        root.join("notes-d.txt"),
        b"-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----\n",
    )?;
    // A control: same shape of name, nothing sensitive inside.
    fs::write(
        root.join("notes-e.txt"),
        b"Ship the redesign in March. Budget fine.\n",
    )?;

    // --- Adversarial filesystem cases --------------------------------------

    // Case collision: distinct on a case-sensitive fs, colliding on APFS default.
    fs::write(root.join("Report.pdf"), b"synthetic\n")?;
    let _ = fs::write(root.join("report.pdf"), b"synthetic\n");

    // Unicode: NFC and NFD spellings of the same visible name.
    fs::write(root.join("café_menu.pdf"), b"synthetic\n")?; // NFC (U+00E9)
    let _ = fs::write(root.join("cafe\u{0301}_notes.pdf"), b"synthetic\n"); // NFD

    // Control characters and a newline in a filename.
    let _ = fs::write(root.join("weird\tname.txt"), b"synthetic\n");

    // A very long name, near NAME_MAX.
    let _ = fs::write(
        root.join(format!("{}.txt", "a".repeat(200))),
        b"synthetic\n",
    );

    // Package directory: must move as one unit, interior never walked.
    let pkg = root.join("Some App.app");
    fs::create_dir_all(pkg.join("Contents/MacOS"))?;
    fs::write(pkg.join("Contents/Info.plist"), b"synthetic\n")?;
    // A sensitive-looking file INSIDE the package. If this ever appears in a
    // plan, the package was walked and the opacity rule is broken.
    fs::write(pkg.join("Contents/MacOS/SSN_card_scan.jpg"), b"synthetic\n")?;

    // Hidden directory: skipped by default.
    let hidden = root.join(".ssh");
    fs::create_dir_all(&hidden)?;
    fs::write(hidden.join("id_rsa"), b"SYNTHETIC\n")?;

    // Nested directory for depth tests.
    let deep = root.join("nested/one/two/three");
    fs::create_dir_all(&deep)?;
    fs::write(deep.join("buried.txt"), b"synthetic\n")?;

    // Outside-the-root target for the escaping symlink.
    let outside = root.parent().unwrap_or(root).join("sweep_fixture_outside");
    fs::create_dir_all(&outside)?;
    fs::write(outside.join("secret_outside.txt"), b"SYNTHETIC\n")?;

    let escaping_symlink = root.join("escape_link");
    #[cfg(unix)]
    {
        let _ = std::os::unix::fs::symlink(&outside, &escaping_symlink);
        // An absolute escape to a system path.
        let _ = std::os::unix::fs::symlink("/etc/passwd", root.join("passwd_link"));
        // A symlink cycle.
        let _ = std::os::unix::fs::symlink(root, root.join("self_link"));
    }

    Ok(Fixture {
        root: root.to_path_buf(),
        sensitive,
        package_dir: pkg,
        escaping_symlink,
    })
}

/// Files whose NAMES are innocent but whose CONTENTS are sensitive.
/// Only content inspection can refuse these.
pub const INNOCENT_NAMES_SENSITIVE_CONTENT: &[&str] =
    &["notes-a.txt", "notes-b.txt", "notes-c.md", "notes-d.txt"];

/// Same shape of name, genuinely harmless content. Must NOT be refused.
pub const INNOCENT_CONTROL: &str = "notes-e.txt";

/// Every filename the generator produces, for output-leak assertions.
pub fn all_names() -> Vec<String> {
    let mut v: Vec<String> = SENSITIVE_NAMES.iter().map(|s| s.to_string()).collect();
    v.extend(ORDINARY_NAMES.iter().map(|s| s.to_string()));
    for i in 4400..4427 {
        v.push(format!("IMG_{i}.HEIC"));
    }
    v
}
