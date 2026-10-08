use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

fn observed_category(output: &Output, category: &str) -> Option<u64> {
    let mut child = Command::new("python3")
        .args(["-c", "import json,sys; data=json.load(sys.stdin); row=next((r for r in data['observations']['categories'] if r['category']==sys.argv[1]),None); print('none' if row is None else row['observed'])", category])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(&output.stdout).unwrap();
    let parsed = child.wait_with_output().unwrap();
    assert!(parsed.status.success());
    let value = String::from_utf8(parsed.stdout).unwrap();
    if value.trim() == "none" { None } else { Some(value.trim().parse().unwrap()) }
}

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "etudes-exact-selection-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("holding")).unwrap();
        Self { root }
    }
    fn file(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stash"));
        command
            .env("ETUDE_STATE_DIR", self.root.join("state"))
            .env("ETUDE_JOURNAL_KEY", "34".repeat(32));
        command
    }
    fn select(&self, sources: &[&Path]) -> Output {
        let mut command = self.command();
        command
            .arg("select")
            .args(sources)
            .arg("--into")
            .arg(self.root.join("holding"))
            .arg("--json");
        command.output().unwrap()
    }
    fn root_after(&self, output: &Output) -> PathBuf {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let entries: Vec<_> = std::fs::read_dir(self.root.join("holding"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(entries.len(), 1);
        entries[0].clone()
    }
    fn pop(&self, root: &Path) -> Output {
        self.command()
            .arg("pop")
            .arg(root)
            .arg("--json")
            .output()
            .unwrap()
    }
    fn holding_empty(&self) {
        assert_eq!(
            std::fs::read_dir(self.root.join("holding"))
                .unwrap()
                .count(),
            0
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn identical_basenames_from_different_parents_restore_without_disclosing_paths() {
    let f = Fixture::new();
    let a = f.file("alpha/tax_return_PRIVATE.pdf", b"synthetic alpha");
    let b = f.file("beta/tax_return_PRIVATE.pdf", b"synthetic beta");
    let untouched = f.file("alpha/unselected_PRIVATE.pdf", b"synthetic untouched");
    f.file(
        "alpha/project.godot",
        b"synthetic marker must not be scanned",
    );
    let output = f.select(&[&a, &b]);
    let holding = f.root_after(&output);
    assert!(!a.exists() && !b.exists());
    assert_eq!(std::fs::read(&untouched).unwrap(), b"synthetic untouched");
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!printed.contains("PRIVATE"));
    assert!(!printed.contains("directory_enumeration"));
    let restored = f.pop(&holding);
    assert!(
        restored.status.success(),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );
    assert_eq!(std::fs::read(a).unwrap(), b"synthetic alpha");
    assert_eq!(std::fs::read(b).unwrap(), b"synthetic beta");
    f.holding_empty();
}

#[test]
fn a_selected_directory_is_an_opaque_object_with_unselected_siblings_untouched() {
    let f = Fixture::new();
    let payload = f.file("chosen/deep/PRIVATE.pdf", b"synthetic tree");
    let sibling = f.file("unselected/PRIVATE.pdf", b"synthetic sibling");
    f.file("chosen/.git/config", b"synthetic marker");
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        f.root.join("missing-target"),
        f.root.join("chosen/dangling-link"),
    )
    .unwrap();
    let output = f.select(&[&f.root.join("chosen")]);
    let holding = f.root_after(&output);
    assert!(!payload.exists());
    assert_eq!(std::fs::read(sibling).unwrap(), b"synthetic sibling");
    let receipt = String::from_utf8_lossy(&output.stdout);
    assert_eq!(observed_category(&output, "fingerprint_bytes"), None);
    assert!(!receipt.contains("directory_enumeration"));
    assert!(!receipt.contains("PRIVATE"));
    assert!(f.pop(&holding).status.success());
    assert_eq!(std::fs::read(payload).unwrap(), b"synthetic tree");
    f.holding_empty();
}

#[test]
fn duplicate_and_parent_child_selections_are_refused_before_holding_effects() {
    let f = Fixture::new();
    let child = f.file("folder/child", b"synthetic child");
    for sources in [
        vec![f.root.join("folder"), child.clone()],
        vec![child.clone(), child.clone()],
    ] {
        let output = f.select(&sources.iter().map(PathBuf::as_path).collect::<Vec<_>>());
        assert_eq!(output.status.code(), Some(2));
        f.holding_empty();
        assert_eq!(std::fs::read(&child).unwrap(), b"synthetic child");
    }
}

#[test]
#[cfg(unix)]
fn hard_link_aliases_are_refused_as_duplicate_object_identities() {
    let f = Fixture::new();
    let original = f.file("original", b"synthetic alias");
    let alias = f.root.join("alias");
    std::fs::hard_link(&original, &alias).unwrap();
    assert_eq!(f.select(&[&original, &alias]).status.code(), Some(2));
    f.holding_empty();
    assert_eq!(std::fs::read(original).unwrap(), b"synthetic alias");
}

#[test]
#[cfg(unix)]
fn valid_and_dangling_selected_links_refuse_without_accessing_the_target() {
    let f = Fixture::new();
    let target = f.file("outside/PRIVATE-target", b"synthetic outside");
    for (name, destination) in [
        ("valid", target.clone()),
        ("dangling", f.root.join("nonexistent")),
    ] {
        let link = f.root.join(name);
        std::os::unix::fs::symlink(&destination, &link).unwrap();
        let output = f.select(&[&link]);
        assert_eq!(output.status.code(), Some(2));
        assert!(
            std::fs::symlink_metadata(link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"synthetic outside");
        f.holding_empty();
        assert_eq!(observed_category(&output, "fingerprint_bytes"), None);
    }
}

#[test]
fn no_journal_selection_refuses_and_empty_stdin_has_no_holding_effects() {
    let f = Fixture::new();
    let source = f.file("source", b"synthetic");
    let output = f
        .command()
        .arg("select")
        .arg(&source)
        .arg("--no-journal")
        .arg("--into")
        .arg(f.root.join("holding"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    f.holding_empty();
    assert!(source.exists());
    let output = f
        .command()
        .args(["select", "--from0", "-", "--json"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    f.holding_empty();
}

#[test]
fn null_stdin_preserves_newlines_and_refuses_malformed_records() {
    let f = Fixture::new();
    let source = f.file("PRIVATE\nrecord", b"synthetic newline");
    let mut command = f.command();
    command
        .args(["select", "--from0", "-", "--json", "--into"])
        .arg(f.root.join("holding"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(source.to_str().unwrap().as_bytes())
        .unwrap();
    input.write_all(&[0]).unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    let holding = f.root_after(&output);
    assert!(f.pop(&holding).status.success());
    assert_eq!(std::fs::read(&source).unwrap(), b"synthetic newline");
    f.holding_empty();
    for malformed in [b"unterminated".as_slice(), b"\0", b"\xff\0"] {
        let list = f.file("bad-list0", malformed);
        let output = f
            .command()
            .arg("select")
            .arg("--from0")
            .arg(list)
            .arg("--into")
            .arg(f.root.join("holding"))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        f.holding_empty();
    }
}

#[test]
fn null_file_input_moves_and_restores_a_real_2048_object_selection() {
    let f = Fixture::new();
    let mut list = Vec::new();
    let mut sources = Vec::new();
    for n in 0..2048 {
        let path = f.file(
            &format!("parent-{}/PRIVATE-{n}.dat", n % 8),
            format!("synthetic {n}").as_bytes(),
        );
        list.extend_from_slice(path.to_str().unwrap().as_bytes());
        list.push(0);
        sources.push(path);
    }
    let list_path = f.file("paths0", &list);
    let output = f
        .command()
        .arg("select")
        .arg("--from0")
        .arg(list_path)
        .arg("--into")
        .arg(f.root.join("holding"))
        .arg("--json")
        .output()
        .unwrap();
    let holding = f.root_after(&output);
    assert!(sources.iter().all(|p| !p.exists()));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE"));
    assert!(f.pop(&holding).status.success());
    for (n, path) in sources.iter().enumerate() {
        assert_eq!(
            std::fs::read(path).unwrap(),
            format!("synthetic {n}").as_bytes()
        );
    }
    f.holding_empty();
}

#[test]
#[cfg(target_os = "macos")]
fn case_and_unicode_aliases_refuse_overlaps_without_changing_stored_names() {
    let f = Fixture::new();
    let source = f.file("CaseFolder/CAFÉ.txt", b"synthetic case");
    let folder_alias = f.root.join("casefolder");
    let child_alias = f.root.join("casefolder/cafe\u{301}.txt");
    let output = f.select(&[&folder_alias, &child_alias]);
    assert_eq!(output.status.code(), Some(2));
    f.holding_empty();
    let output = f.select(&[&child_alias]);
    let holding = f.root_after(&output);
    assert!(f.pop(&holding).status.success());
    assert_eq!(std::fs::read(source).unwrap(), b"synthetic case");
    let names: Vec<_> = std::fs::read_dir(f.root.join("CaseFolder"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("CAFÉ.txt")]);
}

#[test]
fn pop_leaves_unowned_empty_directories_in_holding_root_alone() {
    let f = Fixture::new();
    let source = f.file("source", b"synthetic");
    let output = f.select(&[&source]);
    let holding = f.root_after(&output);
    std::fs::create_dir(holding.join("user-created")).unwrap();
    assert!(f.pop(&holding).status.success());
    assert!(holding.join("user-created").is_dir());
    assert_eq!(std::fs::read(source).unwrap(), b"synthetic");
}

#[test]
fn positional_flag_named_files_do_not_change_receipt_mode_or_journal_policy() {
    let f = Fixture::new();
    let source = f.file("--no-journal", b"synthetic option-shaped filename");
    let output = f
        .command()
        .current_dir(&f.root)
        .args([
            "select",
            "--json",
            "--into",
            "holding",
            "--",
            "--no-journal",
        ])
        .output()
        .unwrap();
    let holding = f.root_after(&output);
    let receipt = String::from_utf8_lossy(&output.stdout);
    assert!(receipt.contains("conditional_journal_restore"));
    assert!(!receipt.contains("manual_restore_required"));
    assert!(f.pop(&holding).status.success());
    assert_eq!(
        std::fs::read(source).unwrap(),
        b"synthetic option-shaped filename"
    );
}

#[test]
#[cfg(unix)]
fn unreadable_selected_file_fails_without_claiming_an_unwritten_journal() {
    use std::os::unix::fs::PermissionsExt;
    let f=Fixture::new();let source=f.file("PRIVATE-unreadable", b"synthetic retained");
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0)).unwrap();
    let output=f.select(&[&source]);assert_eq!(output.status.code(), Some(3));
    let printed=format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(!printed.contains("PRIVATE-unreadable"));assert!(!printed.contains("original paths are retained"));
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(std::fs::read(source).unwrap(), b"synthetic retained");
}
