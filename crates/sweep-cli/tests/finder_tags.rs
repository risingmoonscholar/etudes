//! Finder tags at the command line: a tagged item is left alone and counted,
//! `--include-tagged` is loud and keeps the tag, and the promise is worded as
//! the owner wrote it.
#![cfg(target_os = "macos")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TAG: &str = "com.apple.metadata:_kMDItemUserTags";
const KEY: &str = "0101010101010101010101010101010101010101010101010101010101010101";
const PROMISE: &str = "Sweep decides from filesystem and Finder organization metadata only; it never reads file contents to decide. When it moves a file it reads the first and last 4 KiB, so undo can tell if that file changed.";

struct Scene {
    root: PathBuf,
    state: PathBuf,
}

impl Drop for Scene {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.root.parent().unwrap());
    }
}

fn scene(label: &str) -> Scene {
    let base = std::env::temp_dir().join(format!(
        "sweep-tags-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let root = base.join("root");
    let state = base.join("state");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&state).unwrap();
    for n in 1..=5 {
        fs::write(root.join(format!("document-{n}.pdf")), b"x").unwrap();
    }
    Scene {
        root: root.canonicalize().unwrap(),
        state,
    }
}

fn tag(path: &Path, value: &str) {
    assert!(
        Command::new("xattr")
            .args(["-w", TAG, value])
            .arg(path)
            .status()
            .unwrap()
            .success()
    );
}

fn tag_value(path: &Path) -> Option<String> {
    let out = Command::new("xattr")
        .args(["-p", TAG])
        .arg(path)
        .output()
        .unwrap();
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn sweep(scene: &Scene, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sweep"))
        .env("ETUDE_STATE_DIR", &scene.state)
        .env("ETUDE_JOURNAL_KEY", KEY)
        .args(args)
        .output()
        .unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn a_tagged_item_is_counted_without_its_name_or_path() {
    let s = scene("count");
    tag(&s.root.join("document-2.pdf"), "private-tag-name");
    let root = s.root.to_str().unwrap();

    let human = sweep(&s, &[root, "--since", "0"]);
    let text = out(&human);
    assert!(
        text.contains("1 Finder-tagged item was left alone (tag names not read)"),
        "{text}"
    );
    assert!(text.contains("no contents read to decide"), "{text}");
    assert!(!text.contains("names, sizes and dates only"), "{text}");
    assert!(!text.contains("private-tag-name"), "{text}");
    assert!(!err(&human).contains("private-tag-name"));

    let json = out(&sweep(&s, &[root, "--since", "0", "--json"]));
    assert!(json.contains("\"tagged_skipped\":1"), "{json}");
    assert!(json.contains("\"include_tagged\":false"), "{json}");
    assert!(!json.contains("private-tag-name"), "{json}");
    // The held item's name is not in the agent-facing result at all.
    assert!(!json.contains("document-2.pdf"), "{json}");
}

#[test]
fn apply_leaves_a_tagged_item_in_place_and_reports_the_count() {
    let s = scene("apply");
    let tagged = s.root.join("document-2.pdf");
    tag(&tagged, "kept");
    let root = s.root.to_str().unwrap();
    let applied = sweep(&s, &["apply", root, "--yes", "--since", "0"]);
    assert!(
        applied.status.success(),
        "{}{}",
        out(&applied),
        err(&applied)
    );
    assert!(tagged.exists(), "the tagged item stayed");
    assert_eq!(tag_value(&tagged).as_deref(), Some("kept"));
    assert!(!s.root.join("document-1.pdf").exists(), "the others moved");
    assert!(
        out(&applied).contains("1 Finder-tagged item was left alone"),
        "{}",
        out(&applied)
    );
}

#[test]
fn include_tagged_is_loud_and_the_tag_survives_the_move() {
    let s = scene("include");
    let tagged = s.root.join("document-2.pdf");
    tag(&tagged, "survives");
    let root = s.root.to_str().unwrap();

    for args in [
        vec![root, "--since", "0", "--include-tagged", "--quiet"],
        vec![root, "--since", "0", "--include-tagged", "--json"],
        vec!["review", root, "--include-tagged", "--since", "0"],
    ] {
        let o = sweep(&s, &args);
        let stderr = err(&o);
        assert!(
            stderr.contains("WARNING: --include-tagged is on"),
            "{args:?}: {stderr}"
        );
    }
    let json = out(&sweep(
        &s,
        &[root, "--since", "0", "--include-tagged", "--json"],
    ));
    assert!(json.contains("\"tagged_skipped\":0"), "{json}");
    assert!(json.contains("\"include_tagged\":true"), "{json}");

    let applied = sweep(
        &s,
        &["apply", root, "--yes", "--since", "0", "--include-tagged"],
    );
    assert!(
        applied.status.success(),
        "{}{}",
        out(&applied),
        err(&applied)
    );
    assert!(err(&applied).contains("WARNING: --include-tagged is on"));
    assert!(!tagged.exists(), "the tagged item moved");
    let moved = fs::read_dir(&s.root)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .flat_map(|d| fs::read_dir(d).unwrap().flatten().map(|e| e.path()))
        .find(|p| p.file_name().is_some_and(|n| n == "document-2.pdf"))
        .expect("the tagged file is in a group folder");
    assert_eq!(tag_value(&moved).as_deref(), Some("survives"));

    // Undo brings it home, tag intact.
    let undone = sweep(&s, &["undo"]);
    assert!(undone.status.success(), "{}{}", out(&undone), err(&undone));
    assert_eq!(tag_value(&tagged).as_deref(), Some("survives"));
}

#[test]
fn when_every_item_is_tagged_the_run_has_nothing_to_do() {
    let s = scene("all");
    for n in 1..=5 {
        tag(&s.root.join(format!("document-{n}.pdf")), "t");
    }
    let root = s.root.to_str().unwrap();
    let scan = sweep(&s, &[root, "--since", "0", "--json"]);
    assert_eq!(scan.status.code(), Some(1), "{}", out(&scan));
    assert!(
        out(&scan).contains("\"status\":\"nothing_to_do\""),
        "{}",
        out(&scan)
    );
    assert!(out(&scan).contains("\"tagged_skipped\":5"));
    let human = sweep(&s, &[root, "--since", "0"]);
    assert_eq!(human.status.code(), Some(1));
    let text = out(&human);
    assert!(
        text.contains("5 Finder-tagged items were left alone"),
        "{text}"
    );
    assert!(text.contains("--include-tagged"), "{text}");
    assert!(!text.contains("Nothing here needs organising"), "{text}");
}

#[test]
fn an_item_tagged_after_the_plan_was_exported_is_not_moved() {
    let s = scene("stale");
    let root = s.root.to_str().unwrap();
    let plan = s.state.join("desktop.plan");
    let exported = sweep(
        &s,
        &[
            root,
            "--since",
            "0",
            "--export-plan",
            plan.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(
        exported.status.success(),
        "{}{}",
        out(&exported),
        err(&exported)
    );
    let digest = out(&exported)
        .split("\"plan_digest\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("digest in the result")
        .to_string();
    let tagged = s.root.join("document-2.pdf");
    tag(&tagged, "late");
    let applied = sweep(
        &s,
        &[
            "apply",
            "--plan",
            plan.to_str().unwrap(),
            "--plan-digest",
            &digest,
            "--yes",
        ],
    );
    assert!(!applied.status.success(), "{}", out(&applied));
    assert!(tagged.exists());
    assert!(s.root.join("document-1.pdf").exists(), "nothing moved");
    // An exported plan fixes its policy; the flag cannot override it.
    let overridden = sweep(
        &s,
        &[
            "apply",
            "--plan",
            plan.to_str().unwrap(),
            "--plan-digest",
            &digest,
            "--yes",
            "--include-tagged",
        ],
    );
    assert_eq!(overridden.status.code(), Some(2));
}

#[test]
fn replaying_a_plan_made_with_include_tagged_is_loud_and_moves_the_tagged_item() {
    let s = scene("replay");
    let tagged = s.root.join("document-2.pdf");
    tag(&tagged, "replayed");
    let root = s.root.to_str().unwrap();
    let plan = s.state.join("tagged.plan");
    let exported = sweep(
        &s,
        &[
            root,
            "--since",
            "0",
            "--include-tagged",
            "--export-plan",
            plan.to_str().unwrap(),
            "--json",
        ],
    );
    let digest = out(&exported)
        .split("\"plan_digest\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("digest in the result")
        .to_string();
    let applied = sweep(
        &s,
        &[
            "apply",
            "--plan",
            plan.to_str().unwrap(),
            "--plan-digest",
            &digest,
            "--yes",
        ],
    );
    assert!(
        applied.status.success(),
        "{}{}",
        out(&applied),
        err(&applied)
    );
    assert!(err(&applied).contains("WARNING: --include-tagged is on"));
    assert!(!tagged.exists(), "the plan included the tagged item");
}

#[test]
fn the_promise_is_stated_as_the_owner_worded_it() {
    let s = scene("promise");
    let help = out(&sweep(&s, &["help"]));
    let flat = help.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(flat.contains(PROMISE), "{help}");
    assert!(help.contains("--include-tagged"));
    let verify = out(&sweep(&s, &["verify"]));
    let flat = verify.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("filesystem and Finder organization metadata only; never file contents"),
        "{verify}"
    );
    assert!(flat.contains("it could not return"), "{verify}");
    let readme =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md")).unwrap();
    let flat = readme.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(flat.contains(PROMISE), "README must state the promise");
    assert!(!flat.contains("names, sizes and dates only"));
}
