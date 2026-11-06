use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    base: PathBuf,
    root: PathBuf,
    state: PathBuf,
    key: String,
}
impl Fixture {
    fn new() -> Self {
        let tmp = Command::new("mktemp").arg("-d").output().unwrap();
        assert!(tmp.status.success());
        let base = PathBuf::from(String::from_utf8(tmp.stdout).unwrap().trim());
        let root = base.join("files");
        let state = base.join("state");
        for p in [&root, &state, &base.join("home")] {
            std::fs::create_dir(p).unwrap();
        }
        let key = Command::new("openssl")
            .args(["rand", "-hex", "32"])
            .output()
            .unwrap();
        Self {
            base,
            root,
            state,
            key: String::from_utf8(key.stdout).unwrap().trim().into(),
        }
    }
    fn run(&self, args: &[&str]) -> Output {
        let out = Command::new(env!("CARGO_BIN_EXE_sweep"))
            .args(args)
            .env("HOME", self.base.join("home"))
            .env("ETUDE_STATE_DIR", &self.state)
            .env("ETUDE_JOURNAL_KEY", &self.key)
            .output()
            .unwrap();
        assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
        out
    }
    fn files(&self) {
        for name in ["one.bpy", "two.bpy", "three.bpy", "passport.bpy"] {
            std::fs::write(self.root.join(name), b"synthetic").unwrap();
        }
    }
    fn profile(&self) -> PathBuf {
        self.state.join("sweep-existing-profile.toml")
    }
    fn apply(&self, extra: &[&str]) -> Output {
        let mut args = vec![
            "apply",
            self.root.to_str().unwrap(),
            "--since",
            "0",
            "--yes",
        ];
        args.extend_from_slice(extra);
        self.run(&args)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.base).unwrap();
    }
}

#[test]
fn ungrouped_hint_is_private_structured_and_actionable() {
    let f = Fixture::new();
    f.files();
    let root = f.root.to_str().unwrap();
    let out = f.run(&[root, "--since", "0"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("3 files had no group."), "{text}");
    assert!(text.contains("--map"));
    assert!(!text.contains("one.bpy"));
    let out = f.run(&[root, "--since", "0", "--quiet"]);
    assert!(!String::from_utf8_lossy(&out.stdout).contains("To send"));
    let out = f.run(&[root, "--since", "0", "--json"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"routing_hint\""));
    let out = f.run(&[root, "--since", "0", "--quiet", "--json"]);
    assert!(!String::from_utf8_lossy(&out.stdout).contains("routing_hint"));
    for name in ["single.bpy", "single.pdf", "extensionless"] {
        let small = Fixture::new();
        std::fs::write(small.root.join(name), b"synthetic").unwrap();
        let out = small.run(&[small.root.to_str().unwrap(), "--since", "0"]);
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("1 file had no group."), "{text}");
        assert!(text.contains("sweep propose --help"), "{text}");
        assert!(!text.contains(name));
        assert!(small.run(&["propose", "--help"]).status.success());
    }
}

#[test]
fn save_routes_round_trips_and_preserves_holds() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.files();
    std::fs::create_dir(f.root.join("Blender Files")).unwrap();
    let out = f.apply(&["--map", "bpy=Blender Files", "--save-routes"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(f.root.join("passport.bpy").is_file());
    assert!(f.root.join("Blender Files/one.bpy").is_file());
    assert_eq!(
        std::fs::metadata(f.profile()).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let out = f.run(&[
        f.root.to_str().unwrap(),
        "--scheme",
        "existing",
        "--since",
        "0",
    ]);
    assert_ne!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn saving_refuses_invalid_flags_missing_folders_and_conflicts_before_moves() {
    for extra in [
        vec!["--save-routes"],
        vec!["--save-routes", "--map", "bpy=Box", "--no-journal"],
        vec!["--save-routes", "--map", "bpy=Box", "--scheme", "tags"],
        vec!["--save-routes", "--map", "bpy=Missing"],
    ] {
        let f = Fixture::new();
        f.files();
        let out = f.apply(&extra);
        assert_eq!(out.status.code(), Some(2));
        assert!(f.root.join("one.bpy").exists());
        assert!(!f.profile().exists());
    }
    let f = Fixture::new();
    f.files();
    for folder in ["Box", "Other"] {
        std::fs::create_dir(f.root.join(folder)).unwrap();
    }
    let original = "schema_version = 1\n[[routes]]\nextension = \"bpy\"\nfolder = \"Other\"\n";
    std::fs::write(f.profile(), original).unwrap();
    let out = f.apply(&["--save-routes", "--map", "bpy=Box"]);
    assert_eq!(out.status.code(), Some(2));
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("Box") && text.contains("Other"));
    assert_eq!(std::fs::read_to_string(f.profile()).unwrap(), original);
    assert!(f.root.join("one.bpy").exists());
}

#[test]
fn saved_routes_invalidate_exported_profile_plans() {
    let f = Fixture::new();
    f.files();
    for folder in ["Box", "Reading"] {
        std::fs::create_dir(f.root.join(folder)).unwrap();
    }
    std::fs::write(
        f.profile(),
        "schema_version = 1\n[[routes]]\nextension = \"pdf\"\nfolder = \"Reading\"\n",
    )
    .unwrap();
    for n in 0..3 {
        std::fs::write(f.root.join(format!("{n}.pdf")), b"synthetic").unwrap();
    }
    let plan = f.base.join("export.plan");
    let out = f.run(&[
        f.root.to_str().unwrap(),
        "--since",
        "0",
        "--scheme",
        "existing",
        "--export-plan",
        plan.to_str().unwrap(),
        "--json",
    ]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    let digest = text
        .split("\"plan_digest\":\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let out = f.apply(&["--map", "bpy=Box", "--save-routes"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = f.run(&[
        "apply",
        "--plan",
        plan.to_str().unwrap(),
        "--plan-digest",
        digest,
        "--yes",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stdout).contains("plan_stale"));
}

#[test]
fn failed_apply_saves_nothing_and_says_so() {
    let f = Fixture::new();
    f.files();
    std::fs::create_dir(f.root.join("Box")).unwrap();
    std::fs::write(f.root.join("Box/one.bpy"), b"occupied").unwrap();
    let out = f.apply(&["--map", "bpy=Box", "--save-routes"]);
    assert!(!out.status.success());
    assert!(!f.profile().exists());
    assert!(String::from_utf8_lossy(&out.stderr).contains("routes were not saved"));
    assert!(!f.state.join(".sweep-existing-profile.pending").exists());
}

#[test]
fn help_and_contract_expose_routing() {
    let f = Fixture::new();
    let out = f.run(&["--help"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("--save-routes"));
    let out = f.run(&["contract", "--json"]);
    let text = String::from_utf8(out.stdout).unwrap();
    for word in [
        "--map",
        "--save-routes",
        "--scheme existing",
        "sweep-existing-profile.toml",
        "schema_version = 1",
        "propose",
    ] {
        assert!(text.contains(word), "missing {word}");
    }
}
