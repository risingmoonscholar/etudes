use etude_core::scan::TreeSnapshot;
use std::fs;
use std::path::PathBuf;

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "etude-plan-snapshot-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        fs::write(path.join("first.txt"), b"synthetic fixture").unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn unchanged_tree_validates_without_payload_reads() {
    let fixture = Fixture::new();
    etude_core::scan::reset_receipt();
    let snapshot = TreeSnapshot::capture(&fixture.0, 2).unwrap();
    snapshot.validate().unwrap();
    let receipt = etude_core::scan::receipt_json();
    assert!(!receipt.contains("fingerprint_bytes"));
    assert!(!receipt.contains("file_open"));
    assert!(!receipt.contains("user_bytes"));
}

#[test]
fn added_deleted_replaced_modified_and_root_replacement_refuse() {
    for change in ["add", "delete", "replace", "modify", "root"] {
        let fixture = Fixture::new();
        let snapshot = TreeSnapshot::capture(&fixture.0, 2).unwrap();
        let file = fixture.0.join("first.txt");
        match change {
            "add" => fs::write(fixture.0.join("added.txt"), b"added").unwrap(),
            "delete" => fs::remove_file(file).unwrap(),
            "replace" => {
                fs::rename(&file, fixture.0.join("old.txt")).unwrap();
                fs::write(file, b"synthetic fixture").unwrap();
            }
            "modify" => fs::write(file, b"modified payload").unwrap(),
            "root" => {
                let moved = fixture.0.with_extension("old");
                fs::rename(&fixture.0, &moved).unwrap();
                fs::create_dir(&fixture.0).unwrap();
                fs::write(file, b"synthetic fixture").unwrap();
                fs::remove_dir_all(moved).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(snapshot.validate().is_err(), "accepted {change}");
    }
}

#[test]
fn nested_marker_changes_refuse() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("child")).unwrap();
    let snapshot = TreeSnapshot::capture(&fixture.0, 2).unwrap();
    fs::write(fixture.0.join("child/project.blend"), b"synthetic marker").unwrap();
    assert!(snapshot.validate().is_err());
}

#[cfg(unix)]
#[test]
fn raw_names_survive_and_symlink_targets_are_not_read() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    let fixture = Fixture::new();
    let name = std::ffi::OsString::from_vec(vec![b'x', 0xff]);
    let supports_raw = match fs::write(fixture.0.join(&name), b"synthetic bytes") {
        Ok(()) => true,
        Err(error) if error.raw_os_error() == Some(92) => false,
        Err(error) => panic!("raw name fixture: {error}"),
    };
    symlink("/nonexistent-snapshot-target", fixture.0.join("link")).unwrap();
    let snapshot = TreeSnapshot::capture(&fixture.0, 2).unwrap();
    if supports_raw {
        assert!(
            snapshot
                .entries
                .iter()
                .any(|entry| entry.path == TreeSnapshot::path_commitment(&PathBuf::from(&name)))
        );
    }
    snapshot.validate().unwrap();
}

#[test]
fn hidden_credential_and_package_children_stay_opaque() {
    let fixture = Fixture::new();
    for name in [".ssh", ".hidden", "opaque.app"] {
        fs::create_dir(fixture.0.join(name)).unwrap();
        fs::write(fixture.0.join(name).join("inside"), b"synthetic bytes").unwrap();
    }
    let snapshot = TreeSnapshot::capture(&fixture.0, 3).unwrap();
    assert!(
        snapshot
            .entries
            .iter()
            .all(|entry| entry.path.components().count() == 1)
    );
}

#[test]
fn ancestor_marker_addition_refuses_but_unrelated_sibling_export_does_not() {
    let fixture = Fixture::new();
    let root = fixture.0.join("selected");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file.txt"), b"synthetic bytes").unwrap();
    let snapshot = TreeSnapshot::capture(&root, 1).unwrap();
    fs::write(fixture.0.join("exported-plan.json"), b"synthetic plan").unwrap();
    snapshot.validate().unwrap();
    fs::write(fixture.0.join("Cargo.toml"), b"synthetic marker").unwrap();
    assert!(snapshot.validate().is_err());
}
