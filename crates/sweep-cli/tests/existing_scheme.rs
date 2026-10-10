use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    base: PathBuf,
    root: PathBuf,
    state: PathBuf,
    plan: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!(
            "sweep-existing-scheme-{}-{sequence}",
            std::process::id()
        ));
        let root = base.join("selected");
        let state = base.join("state");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        Self {
            plan: base.join("selected.plan"),
            base,
            root,
            state,
        }
    }

    fn profile(&self, text: &str) {
        std::fs::write(self.state.join("sweep-existing-profile.toml"), text).unwrap();
    }

    fn add_files(&self, extension: &str, count: usize) {
        for index in 0..count {
            std::fs::write(
                self.root.join(format!("synthetic-{index}.{extension}")),
                b"synthetic contents",
            )
            .unwrap();
        }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sweep"))
            .args(arguments)
            .env("ETUDE_STATE_DIR", &self.state)
            .env("ETUDE_JOURNAL_KEY", "42".repeat(32))
            .env("SWEEP_GRACE_SECS", "0")
            .output()
            .unwrap()
    }

    fn export(&self) -> String {
        let output = self.run(&[
            self.root.to_str().unwrap(),
            "--scheme",
            "existing",
            "--since",
            "0",
            "--export-plan",
            self.plan.to_str().unwrap(),
            "--json",
        ]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let tail = stdout
            .split("\"plan_digest\":\"")
            .nth(1)
            .expect("exported result includes its bound plan digest");
        tail.split('"').next().unwrap().to_string()
    }

    fn replay(&self, digest: &str) -> Output {
        self.run(&[
            "apply",
            "--plan",
            self.plan.to_str().unwrap(),
            "--plan-digest",
            digest,
            "--yes",
            "--json",
        ])
    }

    fn source_count(&self, extension: &str) -> usize {
        std::fs::read_dir(&self.root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == extension))
            .count()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn route(extension: &str, folder: &str) -> String {
    format!(
        "schema_version = 1\n\n[[routes]]\nextension = \"{extension}\"\nfolder = \"{folder}\"\n"
    )
}

#[test]
fn existing_profile_routes_builtin_extensions_only_into_a_folder_that_exists() {
    let fixture = Fixture::new();
    let destination = fixture.root.join("Reading");
    std::fs::create_dir(&destination).unwrap();
    fixture.profile(&route("pdf", "Reading"));
    fixture.add_files("pdf", 3);

    let proposed = fixture.run(&[
        fixture.root.to_str().unwrap(),
        "--scheme",
        "existing",
        "--since",
        "0",
        "--json",
    ]);
    assert_eq!(proposed.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&proposed.stdout).contains("\"name\":\"Reading\""));

    let applied = fixture.run(&[
        "apply",
        fixture.root.to_str().unwrap(),
        "--scheme",
        "existing",
        "--since",
        "0",
        "--yes",
        "--json",
    ]);
    assert_eq!(
        applied.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert_eq!(fixture.source_count("pdf"), 0);
    assert_eq!(std::fs::read_dir(destination).unwrap().count(), 3);
}

#[test]
fn missing_destinations_and_ambiguous_profile_routes_leave_sources_untouched() {
    let missing = Fixture::new();
    missing.profile(&route("qoi", "Missing"));
    missing.add_files("qoi", 3);
    let output = missing.run(&[
        missing.root.to_str().unwrap(),
        "--scheme",
        "existing",
        "--since",
        "0",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!missing.root.join("Missing").exists());
    assert_eq!(missing.source_count("qoi"), 3);

    let ambiguous = Fixture::new();
    std::fs::create_dir(ambiguous.root.join("First")).unwrap();
    std::fs::create_dir(ambiguous.root.join("Second")).unwrap();
    ambiguous.profile(
        "schema_version = 1\n\
         [[routes]]\nextension = \"qoi\"\nfolder = \"First\"\n\
         [[routes]]\nextension = \"qoi\"\nfolder = \"Second\"\n",
    );
    ambiguous.add_files("qoi", 3);
    let output = ambiguous.run(&[
        "apply",
        ambiguous.root.to_str().unwrap(),
        "--scheme",
        "existing",
        "--since",
        "0",
        "--yes",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(ambiguous.source_count("qoi"), 3);
    assert_eq!(
        std::fs::read_dir(ambiguous.root.join("First"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        std::fs::read_dir(ambiguous.root.join("Second"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn ordinary_personal_refusal_overrides_an_existing_profile_route() {
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.root.join("Taxes")).unwrap();
    fixture.profile(&route("pdf", "Taxes"));
    for index in 0..3 {
        std::fs::write(
            fixture.root.join(format!("tax_return_{index}.pdf")),
            b"synthetic contents",
        )
        .unwrap();
    }

    let output = fixture.run(&[
        fixture.root.to_str().unwrap(),
        "--scheme",
        "existing",
        "--since",
        "0",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("\"name\":\"Taxes\""));
    assert_eq!(fixture.source_count("pdf"), 3);
    assert_eq!(
        std::fs::read_dir(fixture.root.join("Taxes"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn exported_plan_refuses_a_changed_profile_and_does_not_recreate_a_removed_destination() {
    let changed = Fixture::new();
    std::fs::create_dir(changed.root.join("Reading")).unwrap();
    std::fs::create_dir(changed.root.join("Archive")).unwrap();
    changed.profile(&route("qoi", "Reading"));
    changed.add_files("qoi", 3);
    let digest = changed.export();
    changed.profile(&route("qoi", "Archive"));
    let output = changed.replay(&digest);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("profile changed"));
    assert_eq!(changed.source_count("qoi"), 3);

    let removed = Fixture::new();
    let destination = removed.root.join("Reading");
    std::fs::create_dir(&destination).unwrap();
    removed.profile(&route("qoi", "Reading"));
    removed.add_files("qoi", 3);
    let digest = removed.export();
    std::fs::remove_dir(&destination).unwrap();
    let output = removed.replay(&digest);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        !destination.exists(),
        "replay recreated a removed destination"
    );
    assert_eq!(removed.source_count("qoi"), 3);
}
