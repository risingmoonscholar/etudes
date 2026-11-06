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
    assert!(!String::from_utf8_lossy(&out.stdout).contains("To route"));
    let out = f.run(&[root, "--since", "0", "--json"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"routing_hint\""));
    // Quiet suppresses text only: the structured hint stays for agents.
    let out = f.run(&[root, "--since", "0", "--quiet", "--json"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"routing_hint\""));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("To route"));
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
    let odd = Fixture::new();
    for n in 0..3 {
        std::fs::write(odd.root.join(format!("{n}.-bpy")), b"synthetic").unwrap();
    }
    let out = odd.run(&[odd.root.to_str().unwrap(), "--since", "0"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("sweep propose --help"));
}

#[test]
fn hint_never_names_a_folder_it_found() {
    let f = Fixture::new();
    f.files();
    for folder in ["Archive", "Blender Files"] {
        std::fs::create_dir(f.root.join(folder)).unwrap();
    }
    let root = f.root.to_str().unwrap();
    let text = String::from_utf8(f.run(&[root, "--since", "0"]).stdout).unwrap();
    let hint: Vec<&str> = text.lines().take(3).collect();
    assert_eq!(hint[0], "3 files had no group.", "{text}");
    assert_eq!(
        hint[1], "To route the .bpy files, name a folder after bpy= (sweep creates it if missing):",
        "{text}"
    );
    assert!(
        hint.iter()
            .all(|line| !line.contains("Archive") && !line.contains("Blender")),
        "{text}"
    );
    assert!(
        hint[2] == format!("  sweep --since 0 --map 'bpy=Routed Files' -- {root}")
            || hint[2] == format!("  sweep --since 0 --map 'bpy=Routed Files' -- '{root}'"),
        "{text}"
    );
}

#[test]
fn hint_prints_the_folder_as_typed_and_it_runs_from_there() {
    let f = Fixture::new();
    let home = f.base.join("home");
    let typed = home.join("My Files");
    std::fs::create_dir(&typed).unwrap();
    for name in ["one.bpy", "two.bpy", "three.bpy"] {
        std::fs::write(typed.join(name), b"synthetic").unwrap();
    }
    let run_in = |dir: &std::path::Path, args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_sweep"))
            .args(args)
            .current_dir(dir)
            .env("HOME", &home)
            .env("ETUDE_STATE_DIR", &f.state)
            .env("ETUDE_JOURNAL_KEY", &f.key)
            .output()
            .unwrap();
        assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
        String::from_utf8(out.stdout).unwrap()
    };
    let step = |text: &str| {
        text.lines()
            .find(|line| line.starts_with("  sweep "))
            .unwrap()
            .to_owned()
    };
    let cases = [
        (&home, "~/My Files", "~/'My Files'"),
        (&home, "My Files", "'My Files'"),
        (&typed, "", "."),
    ];
    for (dir, arg, word) in cases {
        let mut args = vec!["--since", "0"];
        if !arg.is_empty() {
            args.push(arg);
        }
        let text = run_in(dir, &args);
        let line = step(&text);
        assert_eq!(
            line,
            format!("  sweep --since 0 --map 'bpy=Routed Files' -- {word}"),
            "{text}"
        );
        assert!(
            text.lines().take(3).all(|l| l.chars().count() < 90),
            "{text}"
        );
        // The printed step, read by a shell in the same folder, plans the files.
        let command =
            line.trim()
                .replacen("sweep", &format!("'{}'", env!("CARGO_BIN_EXE_sweep")), 1);
        let out = Command::new("sh")
            .args(["-c", &command])
            .current_dir(dir)
            .env("HOME", &home)
            .env("ETUDE_STATE_DIR", &f.state)
            .env("ETUDE_JOURNAL_KEY", &f.key)
            .output()
            .unwrap();
        let planned = String::from_utf8(out.stdout).unwrap();
        assert!(
            out.status.success() && planned.contains("Routed Files    3 files"),
            "{planned}"
        );
    }
    // JSON keeps the resolved path, so an agent needs no shell to expand `~`.
    let out = Command::new(env!("CARGO_BIN_EXE_sweep"))
        .args(["--since", "0", "--json", "~/My Files"])
        .current_dir(&home)
        .env("HOME", &home)
        .env("ETUDE_STATE_DIR", &f.state)
        .env("ETUDE_JOURNAL_KEY", &f.key)
        .output()
        .unwrap();
    let json = String::from_utf8(out.stdout).unwrap();
    let resolved = typed.to_str().unwrap();
    assert!(json.contains(&format!("-- '{resolved}'\"")), "{json}");
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
        // A missing folder is missing_input (exit 3); the others are argument errors.
        let missing = extra.contains(&"bpy=Missing");
        assert_eq!(out.status.code(), Some(if missing { 3 } else { 2 }));
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

#[test]
fn a_route_saved_for_another_folder_does_not_block_saving_here() {
    let f = Fixture::new();
    f.files();
    std::fs::create_dir(f.root.join("Box")).unwrap();
    // Models exists only in another tree.
    let original = "schema_version = 1\n[[routes]]\nextension = \"blendx\"\nfolder = \"Models\"\n";
    std::fs::write(f.profile(), original).unwrap();
    let out = f.apply(&["--map", "bpy=Box", "--save-routes"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(f.profile()).unwrap();
    assert!(
        text.starts_with(original) && text.contains("folder = \"Box\""),
        "{text}"
    );
}

#[test]
fn saving_keeps_the_persons_text_and_skips_a_write_with_nothing_new() {
    use std::os::unix::fs::MetadataExt;
    let f = Fixture::new();
    f.files();
    for folder in ["Box", "Other"] {
        std::fs::create_dir(f.root.join(folder)).unwrap();
    }
    let original = "# routes I keep by hand\nschema_version = 1\n\n[[routes]]\nextension = \"bpy\"\nfolder = \"Box\"\n# Blender files\n";
    std::fs::write(f.profile(), original).unwrap();
    let before = std::fs::metadata(f.profile()).unwrap();
    let out = f.apply(&["--map", "bpy=Box", "--save-routes"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("Route .bpy → Box was already saved; the profile was not changed."),
        "{text}"
    );
    let after = std::fs::metadata(f.profile()).unwrap();
    assert_eq!(std::fs::read_to_string(f.profile()).unwrap(), original);
    assert_eq!(
        (before.ino(), before.mtime_nsec()),
        (after.ino(), after.mtime_nsec())
    );
    assert!(f.run(&["undo", f.root.to_str().unwrap()]).status.success());
    for n in 0..3 {
        std::fs::write(f.root.join(format!("{n}.xyz")), b"synthetic").unwrap();
    }
    let out = f.apply(&["--map", "xyz=Other", "--save-routes"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(f.profile()).unwrap();
    assert!(
        text.starts_with(original),
        "the person's comments must stay: {text}"
    );
    assert!(
        text.ends_with("[[routes]]\nextension = \"xyz\"\nfolder = \"Other\"\n"),
        "{text}"
    );
}

#[test]
fn json_reports_the_route_save_outcome() {
    let f = Fixture::new();
    f.files();
    std::fs::create_dir(f.root.join("Box")).unwrap();
    let out = f.apply(&["--map", "bpy=Box", "--save-routes", "--json"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    let profile = f.profile().display().to_string();
    assert!(
        text.contains(&format!(
            "\"routes_saved\":{{\"state\":\"saved\",\"reason\":null,\"added\":[{{\"extension\":\"bpy\",\"folder\":\"Box\"}}],\"requested\":[{{\"extension\":\"bpy\",\"folder\":\"Box\"}}],\"profile\":\"{profile}\",\"written\":true,\"folder_synced\":true}}"
        )),
        "{text}"
    );
    assert!(
        text.contains("\"moved\":3"),
        "the apply details stay in one object: {text}"
    );
    // Refused before anything moved: the same field says so.
    let out = f.apply(&["--map", "bpy=Missing", "--save-routes", "--json"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains(
            "\"routes_saved\":{\"state\":\"not_saved\",\"reason\":\"refused_before_moves\""
        ),
        "{text}"
    );
    assert!(text.contains("\"code\":\"missing_input\""), "{text}");
}

#[test]
fn prepare_problems_have_their_own_codes_and_a_leftover_does_not_block() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.files();
    std::fs::create_dir(f.root.join("Box")).unwrap();
    let json = |out: &Output| String::from_utf8_lossy(&out.stdout).into_owned();
    // A killed run's leftover is removed, and the save goes ahead.
    std::fs::write(f.state.join(".sweep-existing-profile.pending"), b"partial").unwrap();
    let out = f.apply(&["--map", "bpy=Box", "--save-routes"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!f.state.join(".sweep-existing-profile.pending").exists());
    assert!(f.run(&["undo", f.root.to_str().unwrap()]).status.success());
    // An unreadable profile is profile_invalid, naming the profile.
    std::fs::set_permissions(f.profile(), std::fs::Permissions::from_mode(0o000)).unwrap();
    let out = f.apply(&["--map", "bpy=Box", "--save-routes", "--json"]);
    std::fs::set_permissions(f.profile(), std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(json(&out).contains(&format!(
        "\"reason\":{{\"code\":\"profile_invalid\",\"retry\":\"after_change\",\"subject\":{{\"kind\":\"profile\",\"path\":\"{}\"}}}}",
        f.profile().display()
    )), "{}", json(&out));
    assert!(String::from_utf8_lossy(&out.stderr).contains("chmod u+r -- "));
    // A state folder sweep cannot write is permission_denied, with a working step.
    std::fs::remove_file(f.profile()).unwrap();
    std::fs::set_permissions(&f.state, std::fs::Permissions::from_mode(0o500)).unwrap();
    let out = f.apply(&["--map", "bpy=Box", "--save-routes", "--json"]);
    std::fs::set_permissions(&f.state, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(
        json(&out).contains("\"code\":\"permission_denied\""),
        "{}",
        json(&out)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("chmod u+w -- "));
    assert!(f.root.join("one.bpy").exists());
}

#[test]
fn help_says_undo_keeps_saved_routes() {
    let f = Fixture::new();
    let text = String::from_utf8(f.run(&["--help"]).stdout).unwrap();
    assert!(
        text.contains("Undo moves files back but keeps saved routes"),
        "{text}"
    );
}
