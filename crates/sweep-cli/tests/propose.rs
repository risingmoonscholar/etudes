//! `sweep propose` end to end: export, decide, propose, apply, undo.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, SystemTime};

struct Fixture {
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!(
            "sweep-propose-{}-{}-{sequence}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = base.join("selected");
        std::fs::create_dir_all(&root).unwrap();
        for i in 1..=3 {
            std::fs::write(
                root.join(format!("Screenshot 2026-01-0{i} at 10.00.00 AM.png")),
                format!("synthetic screenshot {i}"),
            )
            .unwrap();
        }
        std::fs::write(root.join("report.pdf"), b"synthetic report").unwrap();
        std::fs::write(root.join("notes.xyz"), b"synthetic notes").unwrap();
        std::fs::write(root.join("tax return 2024.pdf"), b"synthetic tax").unwrap();
        std::fs::create_dir(root.join("Reading")).unwrap();
        std::fs::create_dir(root.join("Taxes-2025")).unwrap();
        let root = root.canonicalize().unwrap();
        let old = SystemTime::now() - Duration::from_secs(10 * 86_400);
        for entry in std::fs::read_dir(&root).unwrap().flatten() {
            if entry.file_type().unwrap().is_file() {
                std::fs::File::options()
                    .write(true)
                    .open(entry.path())
                    .unwrap()
                    .set_modified(old)
                    .unwrap();
            }
        }
        Self { base, root }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sweep"))
            .args(arguments)
            .env("ETUDE_STATE_DIR", self.base.join("state"))
            .env("ETUDE_JOURNAL_KEY", "42".repeat(32))
            .env("SWEEP_GRACE_SECS", "0")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn plan(&self) -> PathBuf {
        self.base.join("exported.plan")
    }

    /// Export the plan; returns (stdout, digest).
    fn export(&self) -> (String, String) {
        let out = self.run(&[
            self.root.to_str().unwrap(),
            "--since",
            "0",
            "--export-plan",
            self.plan().to_str().unwrap(),
            "--json",
        ]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        let stdout = text(&out.stdout);
        (stdout.clone(), field_after(&stdout, "\"plan_digest\":\""))
    }

    fn proposal(&self, body: &str) -> PathBuf {
        let path = self.base.join("proposal.json");
        std::fs::write(&path, body).unwrap();
        path
    }

    fn out(&self) -> PathBuf {
        self.base.join("proposed.plan")
    }

    fn propose(&self, digest: &str, proposal: &Path) -> Output {
        self.run(&[
            "propose",
            "--plan",
            self.plan().to_str().unwrap(),
            "--plan-digest",
            digest,
            "--proposal",
            proposal.to_str().unwrap(),
            "--export-plan",
            self.out().to_str().unwrap(),
            "--json",
        ])
    }

    fn apply(&self, digest: &str, extra: &[&str]) -> Output {
        let out = self.out();
        let mut arguments = vec![
            "apply",
            "--plan",
            out.to_str().unwrap(),
            "--plan-digest",
            digest,
            "--yes",
            "--json",
        ];
        arguments.extend_from_slice(extra);
        self.run(&arguments)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn field_after(haystack: &str, marker: &str) -> String {
    haystack
        .split(marker)
        .nth(1)
        .unwrap_or_else(|| panic!("{marker} not in {haystack}"))
        .split('"')
        .next()
        .unwrap()
        .to_string()
}

/// The id of the eligible item with this name, read from the export.
fn id_of(export: &str, name: &str) -> String {
    let at = export
        .find(&format!("\"name\":\"{name}\""))
        .unwrap_or_else(|| panic!("{name} is not in the item list"));
    let head = &export[..at];
    let start = head.rfind("\"id\":\"").unwrap() + 6;
    head[start..].split('"').next().unwrap().to_string()
}

fn body(digest: &str, decisions: &str) -> String {
    format!(
        r#"{{"schema_version":1,"plan_digest":"{digest}","rest":"leave","decisions":[{decisions}]}}"#
    )
}

fn mv(id: &str, folder: &str) -> String {
    format!(r#"{{"item":"{id}","folder":"{folder}","mode":"require_existing"}}"#)
}

fn listing(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut rows = Vec::new();
    fn walk(dir: &Path, root: &Path, rows: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().display().to_string();
            if entry.file_type().unwrap().is_dir() {
                rows.push((relative, Vec::new()));
                walk(&path, root, rows);
            } else {
                rows.push((relative, std::fs::read(&path).unwrap()));
            }
        }
    }
    walk(root, root, &mut rows);
    rows.sort();
    rows
}

fn reason(output: &Output) -> String {
    let stdout = text(&output.stdout);
    stdout
        .split("\"reason\":")
        .last()
        .unwrap()
        .trim_end_matches('}')
        .trim_end()
        .to_string()
        + "}"
}

fn refused_with(output: &Output, code: &str, retry: &str, subject: Option<&str>) {
    assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
    let expected = match subject {
        Some(kind) => {
            format!(r#"{{"code":"{code}","retry":"{retry}","subject":{{"kind":"{kind}"}}}}"#)
        }
        None => format!(r#"{{"code":"{code}","retry":"{retry}"}}"#),
    };
    assert!(
        reason(output).starts_with(&expected),
        "expected {expected}, got {}",
        reason(output)
    );
    assert!(text(&output.stdout).contains("\"status\":\"refused\""));
}

#[test]
fn export_lists_eligible_items_with_name_size_and_group() {
    let fixture = Fixture::new();
    let (stdout, _) = fixture.export();
    let items = stdout.split("\"items\":").nth(1).expect("items in details");
    assert!(items.contains(
        r#"{"id":"i-000000","name":"Screenshot 2026-01-01 at 10.00.00 AM.png","size":22,"group":"Screenshots"}"#
    ));
    assert!(items.contains(r#"{"id":"i-000004","name":"report.pdf","size":16,"group":null}"#));
    // A personal-looking file is counted and never listed.
    assert!(stdout.contains("\"looks_personal\":1"));
    assert!(!items.contains("tax return"));
}

#[test]
fn propose_apply_and_undo_restore_every_path_and_byte() {
    let fixture = Fixture::new();
    let (export, digest) = fixture.export();
    let before = listing(&fixture.root);
    let report = id_of(&export, "report.pdf");
    let shot = id_of(&export, "Screenshot 2026-01-01 at 10.00.00 AM.png");
    let notes = id_of(&export, "notes.xyz");
    let proposal = fixture.proposal(&body(
        &digest,
        &format!(
            r#"{},{},{{"item":"{notes}","skip":true}}"#,
            mv(&report, "Reading"),
            mv(&shot, "Taxes-2025"),
        ),
    ));
    let proposed = fixture.propose(&digest, &proposal);
    assert_eq!(
        proposed.status.code(),
        Some(0),
        "{}",
        text(&proposed.stderr)
    );
    let stdout = text(&proposed.stdout);
    assert!(stdout.contains("\"status\":\"done\""));
    assert!(stdout.contains("\"moves\":2,\"skipped\":1"));
    assert!(stdout.contains("\"operation\":\"propose\""));
    // Propose moves nothing.
    assert_eq!(listing(&fixture.root), before);
    let new_digest = field_after(&stdout, "\"plan_digest\":\"");
    assert_ne!(new_digest, digest);

    let applied = fixture.apply(&new_digest, &[]);
    assert_eq!(applied.status.code(), Some(0), "{}", text(&applied.stderr));
    assert!(fixture.root.join("Reading/report.pdf").is_file());
    assert!(
        fixture
            .root
            .join("Taxes-2025/Screenshot 2026-01-01 at 10.00.00 AM.png")
            .is_file()
    );
    // A skipped item, an unnamed item and an ineligible file did not move.
    for stays in [
        "notes.xyz",
        "Screenshot 2026-01-02 at 10.00.00 AM.png",
        "tax return 2024.pdf",
    ] {
        assert!(fixture.root.join(stays).is_file(), "{stays} moved");
    }

    let undone = fixture.run(&["undo", "--json"]);
    assert_eq!(undone.status.code(), Some(0), "{}", text(&undone.stderr));
    assert_eq!(listing(&fixture.root), before);
}

#[test]
fn a_guessed_id_for_a_personal_looking_file_is_unknown_item() {
    let fixture = Fixture::new();
    let (_, digest) = fixture.export();
    for guess in ["i-000005", "i-000006", "i-000007", "i-000100"] {
        let proposal = fixture.proposal(&body(&digest, &mv(guess, "Taxes-2025")));
        let out = fixture.propose(&digest, &proposal);
        refused_with(&out, "unknown_item", "never", Some("item"));
        assert!(!fixture.out().exists());
    }
    assert!(fixture.root.join("tax return 2024.pdf").is_file());
}

#[test]
fn an_item_named_twice_is_refused() {
    let fixture = Fixture::new();
    let (export, digest) = fixture.export();
    let id = id_of(&export, "report.pdf");
    let proposal = fixture.proposal(&body(
        &digest,
        &format!(r#"{},{{"item":"{id}","skip":true}}"#, mv(&id, "Reading")),
    ));
    let out = fixture.propose(&digest, &proposal);
    refused_with(&out, "item_assigned_twice", "never", Some("item"));
    assert!(!fixture.out().exists());
}

#[test]
fn an_edited_plan_file_is_refused_by_the_digest_check() {
    let fixture = Fixture::new();
    let (export, digest) = fixture.export();
    let proposal = fixture.proposal(&body(
        &digest,
        &mv(&id_of(&export, "report.pdf"), "Reading"),
    ));
    let mut bytes = std::fs::read(fixture.plan()).unwrap();
    let at = bytes.len() / 2;
    bytes[at] ^= 1;
    std::fs::write(fixture.plan(), bytes).unwrap();
    let out = fixture.propose(&digest, &proposal);
    refused_with(&out, "plan_stale", "after_change", Some("plan"));
    assert!(!fixture.out().exists());
}

#[test]
fn an_edited_proposal_digest_is_refused_stale() {
    let fixture = Fixture::new();
    let (export, digest) = fixture.export();
    let edited = format!("{}0", &digest[..63]);
    let edited = if edited == digest {
        format!("{}1", &digest[..63])
    } else {
        edited
    };
    let proposal = fixture.proposal(&body(
        &edited,
        &mv(&id_of(&export, "report.pdf"), "Reading"),
    ));
    let out = fixture.propose(&digest, &proposal);
    refused_with(&out, "plan_stale", "after_change", Some("plan"));
    assert!(!fixture.out().exists());
}

#[test]
fn a_changed_tree_is_refused_stale() {
    let fixture = Fixture::new();
    let (export, digest) = fixture.export();
    let proposal = fixture.proposal(&body(
        &digest,
        &mv(&id_of(&export, "report.pdf"), "Reading"),
    ));
    std::fs::write(fixture.root.join("arrived later.txt"), b"synthetic").unwrap();
    let out = fixture.propose(&digest, &proposal);
    refused_with(&out, "plan_stale", "after_change", Some("plan"));
    assert!(!fixture.out().exists());
}

#[test]
fn a_folder_swapped_for_a_link_between_propose_and_apply_is_refused() {
    let fixture = Fixture::new();
    let (export, digest) = fixture.export();
    let proposal = fixture.proposal(&body(
        &digest,
        &mv(&id_of(&export, "report.pdf"), "Reading"),
    ));
    let proposed = fixture.propose(&digest, &proposal);
    assert_eq!(
        proposed.status.code(),
        Some(0),
        "{}",
        text(&proposed.stderr)
    );
    let new_digest = field_after(&text(&proposed.stdout), "\"plan_digest\":\"");
    let elsewhere = fixture.base.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::remove_dir(fixture.root.join("Reading")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, fixture.root.join("Reading")).unwrap();
    let applied = fixture.apply(&new_digest, &[]);
    assert_eq!(applied.status.code(), Some(2), "{}", text(&applied.stderr));
    assert!(text(&applied.stdout).contains("\"status\":\"refused\""));
    assert!(fixture.root.join("report.pdf").is_file());
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn one_bad_decision_writes_no_plan() {
    let fixture = Fixture::new();
    let (export, digest) = fixture.export();
    let good = mv(&id_of(&export, "report.pdf"), "Reading");
    let also = mv(&id_of(&export, "notes.xyz"), "Taxes-2025");
    let bad = mv("i-000042", "Reading");
    let before = listing(&fixture.root);
    for decisions in [
        format!("{good},{bad},{also}"),
        format!("{bad},{good}"),
        format!(
            "{good},{also},{}",
            mv(&id_of(&export, "report.pdf"), "Taxes-2025")
        ),
    ] {
        let proposal = fixture.proposal(&body(&digest, &decisions));
        let out = fixture.propose(&digest, &proposal);
        assert_eq!(out.status.code(), Some(2), "{decisions}");
        assert!(!fixture.out().exists(), "a refused proposal wrote a plan");
    }
    assert_eq!(listing(&fixture.root), before);
}

#[test]
fn rest_must_be_present_and_leave() {
    let fixture = Fixture::new();
    let (export, digest) = fixture.export();
    let decision = mv(&id_of(&export, "report.pdf"), "Reading");
    for text in [
        format!(r#"{{"schema_version":1,"plan_digest":"{digest}","decisions":[{decision}]}}"#),
        format!(
            r#"{{"schema_version":1,"plan_digest":"{digest}","rest":"keep_plan","decisions":[{decision}]}}"#
        ),
        format!(
            r#"{{"schema_version":1,"plan_digest":"{digest}","rest":"","decisions":[{decision}]}}"#
        ),
    ] {
        let proposal = fixture.proposal(&text);
        refused_with(
            &fixture.propose(&digest, &proposal),
            "invalid_arguments",
            "never",
            None,
        );
        assert!(!fixture.out().exists());
    }
}

#[test]
fn there_is_no_create_if_absent() {
    let fixture = Fixture::new();
    let (export, digest) = fixture.export();
    let id = id_of(&export, "report.pdf");
    let proposal = fixture.proposal(&body(
        &digest,
        &format!(r#"{{"item":"{id}","folder":"Fresh","mode":"create_if_absent"}}"#),
    ));
    refused_with(
        &fixture.propose(&digest, &proposal),
        "invalid_arguments",
        "never",
        None,
    );
    assert!(!fixture.root.join("Fresh").exists());
}

#[test]
fn size_and_count_limits_are_invalid_arguments() {
    let fixture = Fixture::new();
    let (export, digest) = fixture.export();
    let id = id_of(&export, "report.pdf");
    let many = vec![format!(r#"{{"item":"{id}","skip":true}}"#); 5_001].join(",");
    let proposal = fixture.proposal(&body(&digest, &many));
    refused_with(
        &fixture.propose(&digest, &proposal),
        "invalid_arguments",
        "never",
        None,
    );
    let big = format!(
        "{}{}",
        body(&digest, &mv(&id, "Reading")),
        " ".repeat(1024 * 1024)
    );
    let proposal = fixture.proposal(&big);
    refused_with(
        &fixture.propose(&digest, &proposal),
        "invalid_arguments",
        "never",
        None,
    );
    let proposal = fixture.proposal("this is not json");
    refused_with(
        &fixture.propose(&digest, &proposal),
        "invalid_arguments",
        "never",
        None,
    );
    assert!(!fixture.out().exists());
}

#[test]
fn folder_rules_use_the_existing_codes() {
    let fixture = Fixture::new();
    std::fs::write(fixture.root.join("Occupied"), b"synthetic").unwrap();
    // The new file changes the tree, so export after making it.
    let (export, digest) = fixture.export();
    let id = id_of(&export, "report.pdf");
    let cases = [
        ("..", "destination_outside_root", "destination"),
        ("a/b", "destination_outside_root", "destination"),
        ("/tmp", "destination_outside_root", "destination"),
        ("", "destination_outside_root", "destination"),
        ("Absent", "profile_missing", "profile"),
        ("Occupied", "destination_exists", "destination"),
    ];
    for (folder, code, subject) in cases {
        let proposal = fixture.proposal(&body(&digest, &mv(&id, folder)));
        refused_with(
            &fixture.propose(&digest, &proposal),
            code,
            "after_change",
            Some(subject),
        );
        assert!(!fixture.out().exists(), "{folder}");
    }
    assert!(!fixture.root.join("Absent").exists());
}

#[test]
fn a_proposed_plan_requires_a_journal() {
    let fixture = Fixture::new();
    let (export, digest) = fixture.export();
    let proposal = fixture.proposal(&body(
        &digest,
        &mv(&id_of(&export, "report.pdf"), "Reading"),
    ));
    let proposed = fixture.propose(&digest, &proposal);
    let new_digest = field_after(&text(&proposed.stdout), "\"plan_digest\":\"");
    let applied = fixture.apply(&new_digest, &["--no-journal"]);
    assert_eq!(applied.status.code(), Some(2));
    assert!(fixture.root.join("report.pdf").is_file());
}

#[test]
fn propose_takes_its_four_inputs_and_no_path() {
    let fixture = Fixture::new();
    let (_, digest) = fixture.export();
    let out = fixture.run(&[
        "propose",
        "--plan",
        fixture.plan().to_str().unwrap(),
        "--json",
    ]);
    refused_with(&out, "invalid_arguments", "never", None);
    let proposal = fixture.proposal("{}");
    let out = fixture.run(&[
        "propose",
        fixture.root.to_str().unwrap(),
        "--plan",
        fixture.plan().to_str().unwrap(),
        "--plan-digest",
        &digest,
        "--proposal",
        proposal.to_str().unwrap(),
        "--export-plan",
        fixture.out().to_str().unwrap(),
        "--json",
    ]);
    refused_with(&out, "invalid_arguments", "never", None);
    let out = fixture.propose(&digest, &fixture.base.join("absent.json"));
    assert_eq!(out.status.code(), Some(3));
    assert!(reason(&out).contains("\"code\":\"missing_input\""));
}

#[test]
fn the_contract_lists_the_two_item_codes_and_the_propose_operation() {
    let fixture = Fixture::new();
    let out = fixture.run(&["contract", "--json"]);
    let stdout = text(&out.stdout);
    assert!(stdout.contains(
        r#"{"code":"unknown_item","retry":"never","subject_kind":"item","operations":["propose"]"#
    ));
    assert!(stdout.contains(
        r#"{"code":"item_assigned_twice","retry":"never","subject_kind":"item","operations":["propose"]"#
    ));
    assert!(stdout.contains(r#"{"operation":"propose","reads":["#));
    assert!(stdout.contains("\"schema_version\":12"));
}
