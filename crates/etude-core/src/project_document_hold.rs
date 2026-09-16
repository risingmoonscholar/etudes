use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new(label: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("document_hold_{label}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn file(&self, path: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"fixture").unwrap();
    }
    fn scan(&self, whole_units: bool) -> ScanOutcome {
        scan(
            &self.0,
            &ScanConfig {
                depth: 4,
                whole_units,
                grace: None,
                ..Default::default()
            },
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn siblings_are_shared_and_the_holder_is_deterministic() {
    let f = Fixture::new("siblings");
    f.file("scenes/b.blend");
    f.file("scenes/a.blend");
    f.file("textures/a.png");
    f.file("invoice.pdf");
    for whole in [false, true] {
        let out = f.scan(whole);
        assert_eq!(out.entries.len(), 1);
        assert_eq!(out.entries[0].name, "invoice.pdf");
        assert!(
            out.project_holds.iter().any(|(_, reason)| *reason
                == crate::Untouched::NearProjectDocument("scenes/a.blend".into()))
        );
    }
}

#[test]
fn two_documents_beside_textures_in_one_child_hold_locally() {
    let f = Fixture::new("local");
    f.file("project/b.blend");
    f.file("project/a.blend");
    f.file("project/textures/a.png");
    f.file("invoice.pdf");
    for whole in [false, true] {
        let out = f.scan(whole);
        assert_eq!(out.entries.len(), 1);
        assert_eq!(out.entries[0].name, "invoice.pdf");
    }
    let inner = scan(
        &f.0.join("project"),
        &ScanConfig {
            depth: 4,
            grace: None,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        inner
            .project_holds
            .iter()
            .any(|(_, reason)| *reason == crate::Untouched::NearProjectDocument("a.blend".into()))
    );
}

#[test]
fn a_loose_document_does_not_hold_invoices_or_screenshots() {
    let f = Fixture::new("loose");
    f.file("a.blend");
    f.file("invoice.pdf");
    f.file("Screenshot 2026-08-12 at 9.15.11 AM.png");
    f.file("sound.wav");
    for whole in [false, true] {
        let out = f.scan(whole);
        assert_eq!(out.entries.len(), 2);
        assert!(out.entries.iter().any(|e| e.name == "invoice.pdf"));
    }
}

#[test]
#[cfg(unix)]
fn unreadable_folder_and_marker_count_sibling_assets() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new("unreadable");
    f.file("scenes/a.blend");
    f.file("textures/a.png");
    f.file("invoice.pdf");
    for path in ["scenes", "scenes/a.blend"] {
        let path = f.0.join(path);
        let original = fs::metadata(&path).unwrap().permissions();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        for whole in [false, true] {
            let out = f.scan(whole);
            // Restore even before asserting so a failure leaves no locked fixture.
            assert_eq!(
                out.entries
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect::<Vec<_>>(),
                ["invoice.pdf"]
            );
            assert!(out.skipped_unreadable >= 1, "{out:?}");
        }
        fs::set_permissions(path, original).unwrap();
    }
}

#[test]
#[cfg(unix)]
fn external_asset_link_is_held_without_following_it() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new("link");
    f.file("root/scenes/a.blend");
    f.file("assets/a.png");
    symlink("../assets", f.0.join("root/textures")).unwrap();
    for whole in [false, true] {
        let out = scan(
            &f.0.join("root"),
            &ScanConfig {
                whole_units: whole,
                depth: 4,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(out.entries.is_empty());
        assert!(f.0.join("root/textures/a.png").is_file());
    }
}

#[test]
fn disappearing_child_restored_before_traversal_still_holds_assets() {
    for whole in [false, true] {
        let f = Fixture::new(if whole {
            "race_whole"
        } else {
            "race_recursive"
        });
        f.file("scenes/a.blend");
        f.file("textures/a.png");
        f.file("invoice.pdf");
        let scenes = f.0.join("scenes");
        let parked = f.0.with_extension("parked");
        let (from, to) = (scenes.clone(), parked.clone());
        BEFORE_CHILD_DOCUMENT_PROBE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |_| {
                fs::rename(from, to).unwrap();
            }))
        });
        AFTER_DOCUMENT_PROBE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |_| {
                fs::rename(parked, scenes).unwrap();
            }))
        });
        let out = f.scan(whole);
        assert!(
            out.skipped_unreadable > 0,
            "the absent child must fail closed"
        );
        assert_eq!(out.entries.len(), 1, "{out:?}");
        assert_eq!(out.entries[0].name, "invoice.pdf");
    }
}

#[test]
fn holder_appearing_after_prescan_retracts_earlier_assets() {
    for whole in [false, true] {
        let f = Fixture::new(if whole {
            "late_whole"
        } else {
            "late_recursive"
        });
        f.file("a_textures/a.png");
        fs::create_dir(f.0.join("z_scenes")).unwrap();
        f.file("invoice.pdf");
        let marker = f.0.join("z_scenes/a.blend");
        AFTER_DOCUMENT_PROBE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |_| {
                fs::write(marker, b"new document").unwrap();
            }))
        });
        let out = f.scan(whole);
        assert_eq!(out.entries.len(), 1, "{out:?}");
        assert_eq!(out.entries[0].name, "invoice.pdf");
    }
}
