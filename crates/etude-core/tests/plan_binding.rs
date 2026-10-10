use etude_core::{
    apply::{self, ApplyError},
    plan::{self, BindingContext, BoundPlan},
    scan::{self, ScanConfig},
};
use std::path::PathBuf;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "etudes-bound-{}-{}-{sequence}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        for i in 0..4 {
            std::fs::write(
                root.join(format!("Screenshot 2026-01-01 at 10.0{i}.png")),
                b"synthetic payload",
            )
            .unwrap();
        }
        Self(root)
    }
    fn context() -> BindingContext {
        BindingContext::new("test", "1.0", "scheme-v1", "metadata-contract-v1")
    }
    fn plan(&self) -> BoundPlan {
        let out = scan::scan(
            &self.0,
            &ScanConfig {
                grace: None,
                ..Default::default()
            },
        )
        .unwrap();
        let mut proposal = plan::build(&out);
        for group in &mut proposal.groups {
            group.accepted = true;
        }
        BoundPlan::from_plan(proposal, Self::context()).unwrap()
    }
    fn export_path(&self) -> PathBuf {
        self.0.with_extension("plan")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
        let _ = std::fs::remove_file(self.export_path());
    }
}

#[test]
fn altered_tree_cannot_apply_against_exported_original_plan() {
    for change in ["add", "replace", "edit", "marker"] {
        let fixture = Fixture::new();
        let bound = fixture.plan();
        let file = fixture.export_path();
        bound.export(&file).unwrap();
        let replay = BoundPlan::load(&file, bound.digest()).unwrap();
        let source = replay.groups[0].members[0].clone();
        match change {
            "add" => std::fs::write(fixture.0.join("new.txt"), b"new").unwrap(),
            "replace" => {
                std::fs::remove_file(&source).unwrap();
                std::fs::write(&source, b"replacement").unwrap();
            }
            "edit" => std::fs::write(&source, b"changed bytes").unwrap(),
            _ => std::fs::write(fixture.0.join("Cargo.toml"), b"[package]").unwrap(),
        }
        let before = std::fs::read_dir(&fixture.0).unwrap().count();
        let metadata = std::fs::metadata(&fixture.0).unwrap();
        let err = apply::apply(&replay, &Fixture::context(), None, None).unwrap_err();
        assert!(matches!(err, ApplyError::StalePlan(_)), "{change}: {err}");
        assert!(err.to_string().contains("replan required"));
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), before);
        assert_eq!(
            std::fs::metadata(&fixture.0).unwrap().modified().unwrap(),
            metadata.modified().unwrap(),
            "case-fold probe must not run"
        );
        assert!(source.exists());
    }
}

#[test]
fn root_replacement_refuses_before_any_new_destination() {
    let fixture = Fixture::new();
    let bound = fixture.plan();
    let moved = fixture.0.with_extension("old");
    std::fs::rename(&fixture.0, &moved).unwrap();
    std::fs::create_dir(&fixture.0).unwrap();
    let err = apply::apply(&bound, &Fixture::context(), None, None).unwrap_err();
    assert!(matches!(err, ApplyError::StalePlan(_)));
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 0);
    std::fs::remove_dir_all(moved).unwrap();
}

#[test]
fn every_context_component_is_independently_required() {
    let fixture = Fixture::new();
    let bound = fixture.plan();
    for field in 0..4 {
        let mut expected = Fixture::context();
        match field {
            0 => expected.tool = "other".into(),
            1 => expected.tool_version = "2.0".into(),
            2 => expected.profile = "scheme-v2".into(),
            _ => expected.observation_contract = "metadata-contract-v2".into(),
        }
        assert!(matches!(
            apply::apply(&bound, &expected, None, None),
            Err(ApplyError::StalePlan(_))
        ));
    }
}

#[test]
fn acceptance_and_rename_keep_the_original_observations() {
    let fixture = Fixture::new();
    let mut bound = fixture.plan();
    let first = bound.digest().to_string();
    bound.groups[0].name = "Chosen".into();
    bound.finalize_choices().unwrap();
    assert_ne!(first, bound.digest());
    let source = bound.groups[0].members[0].clone();
    std::fs::write(&source, b"changed").unwrap();
    bound.groups[0].name = "Another".into();
    assert!(bound.finalize_choices().is_err());
    assert!(matches!(
        apply::apply(&bound, &Fixture::context(), None, None),
        Err(ApplyError::StalePlan(_))
    ));
}

#[test]
fn byte_tampering_and_wrong_digest_refuse_without_recapture() {
    let fixture = Fixture::new();
    let bound = fixture.plan();
    let path = fixture.export_path();
    bound.export(&path).unwrap();
    assert!(BoundPlan::load(&path, &"0".repeat(64)).is_err());
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[20] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    assert!(BoundPlan::load(&path, bound.digest()).is_err());
}

#[test]
fn read_only_proposal_has_no_mutating_binding_constructor() {
    let proposal = plan::Plan::display_only(PathBuf::from("/fixture"), Vec::new());
    assert!(BoundPlan::from_plan(proposal, Fixture::context()).is_err());
}

#[test]
fn exports_commit_held_names_and_enforce_private_creation() {
    let fixture = Fixture::new();
    let hidden_name = "2024-1099-INT.pdf";
    std::fs::write(fixture.0.join(hidden_name), b"synthetic sensitive fixture").unwrap();
    let bound = fixture.plan();
    let path = fixture.export_path();
    bound.export(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(
        !bytes
            .windows(hidden_name.len())
            .any(|part| part == hidden_name.as_bytes())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(bound.export(&path).is_err());
    assert!(bound.export(&fixture.0.join("inside.plan")).is_err());
    let loaded = BoundPlan::load(&path, bound.digest()).unwrap();
    loaded.validate(&Fixture::context()).unwrap();
}

#[test]
fn unchanged_import_applies_exact_selected_members() {
    let fixture = Fixture::new();
    let bound = fixture.plan();
    let members = bound.groups[0].members.clone();
    let destination = bound.groups[0].name.clone();
    let file = fixture.export_path();
    bound.export(&file).unwrap();
    let replay = BoundPlan::load(&file, bound.digest()).unwrap();
    let report = apply::apply(&replay, &Fixture::context(), None, None).unwrap();
    assert_eq!(report.moved, members.len());
    for source in members {
        assert!(!source.exists());
        assert!(
            fixture
                .0
                .join(&destination)
                .join(source.file_name().unwrap())
                .exists()
        );
    }
}

#[test]
fn immediate_apply_keeps_open_inode_while_exported_replay_remains_strict() {
    let fixture = Fixture::new();
    let out = scan::scan_for_live_apply(
        &fixture.0,
        &ScanConfig {
            grace: None,
            ..Default::default()
        },
    )
    .unwrap();
    let mut proposal = plan::build(&out);
    for group in &mut proposal.groups {
        group.accepted = true;
    }
    let bound = BoundPlan::from_live_plan(proposal, Fixture::context()).unwrap();
    let source = bound.groups[0].members[0].clone();
    let mut writer = std::fs::OpenOptions::new()
        .append(true)
        .open(&source)
        .unwrap();
    use std::io::Write;
    writer.write_all(b" appended while open").unwrap();
    assert!(bound.export(&fixture.export_path()).is_err());
    let destination = fixture
        .0
        .join(&bound.groups[0].name)
        .join(source.file_name().unwrap());
    apply::apply(&bound, &Fixture::context(), None, None).unwrap();
    writer.write_all(b" after move").unwrap();
    assert_eq!(
        std::fs::read(destination).unwrap(),
        b"synthetic payload appended while open after move"
    );
}

#[test]
fn immediate_apply_still_refuses_replaced_sources_and_new_markers() {
    for marker in [false, true] {
        let fixture = Fixture::new();
        let out = scan::scan_for_live_apply(
            &fixture.0,
            &ScanConfig {
                grace: None,
                ..Default::default()
            },
        )
        .unwrap();
        let mut proposal = plan::build(&out);
        for group in &mut proposal.groups {
            group.accepted = true;
        }
        let bound = BoundPlan::from_live_plan(proposal, Fixture::context()).unwrap();
        if marker {
            std::fs::write(fixture.0.join("Cargo.toml"), b"synthetic marker").unwrap();
        } else {
            let source = &bound.groups[0].members[0];
            std::fs::rename(source, fixture.0.with_extension("saved")).unwrap();
            std::fs::write(source, b"replacement").unwrap();
            std::fs::remove_file(fixture.0.with_extension("saved")).unwrap();
        }
        assert!(matches!(
            apply::apply(&bound, &Fixture::context(), None, None),
            Err(ApplyError::StalePlan(_))
        ));
        assert!(!fixture.0.join(&bound.groups[0].name).exists());
    }
}
