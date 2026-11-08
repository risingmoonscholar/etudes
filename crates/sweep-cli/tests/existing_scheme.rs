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
fn missing_builtin_route_does_not_fall_back_to_documents() {
    let fixture = Fixture::new();
    fixture.profile(&route("pdf", "Reading"));
    fixture.add_files("pdf", 3);

    let output = fixture.run(&[
        "apply",
        fixture.root.to_str().unwrap(),
        "--scheme",
        "existing",
        "--since",
        "0",
        "--yes",
        "--json",
    ]);

    // The saved route is the person's destination: sweep makes the missing
    // folder and the PDFs go there, never into the built-in Documents.
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fixture.source_count("pdf"), 0);
    assert_eq!(
        std::fs::read_dir(fixture.root.join("Reading"))
            .unwrap()
            .count(),
        3
    );
    assert!(!fixture.root.join("Documents").exists());
}

#[test]
fn a_missing_route_folder_is_listed_created_and_removed_again_by_undo() {
    let fixture = Fixture::new();
    fixture.profile(&route("qoi", "Pictures Of Mine"));
    let mut originals = Vec::new();
    for index in 0..3 {
        let name = format!("synthetic-{index}.qoi");
        let bytes = format!("synthetic qoi payload {index}").into_bytes();
        std::fs::write(fixture.root.join(&name), &bytes).unwrap();
        originals.push((name, bytes));
    }
    let destination = fixture.root.join("Pictures Of Mine");
    let root = fixture.root.to_str().unwrap();

    let planned = fixture.run(&[root, "--scheme", "existing", "--since", "0"]);
    assert_eq!(planned.status.code(), Some(0));
    let text = String::from_utf8_lossy(&planned.stdout);
    assert!(
        text.contains("sweep creates the folder Pictures Of Mine for your saved route"),
        "{text}"
    );
    assert!(
        text.contains(".qoi (saved route, new folder sweep creates)"),
        "{text}"
    );
    assert!(!text.contains("chosen by your agent"), "{text}");
    let planned = fixture.run(&[root, "--scheme", "existing", "--since", "0", "--json"]);
    assert_eq!(planned.status.code(), Some(0));
    let json = String::from_utf8_lossy(&planned.stdout);
    assert!(
        json.contains(
            "\"name\":\"Pictures Of Mine\",\"signal\":\".qoi (saved route, new folder sweep creates)\""
        ),
        "{json}"
    );
    assert!(!destination.exists(), "planning created the folder");

    let applied = fixture.run(&[
        "apply", root, "--scheme", "existing", "--since", "0", "--yes", "--json",
    ]);
    assert_eq!(
        applied.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert_eq!(fixture.source_count("qoi"), 0);
    for (name, bytes) in &originals {
        assert_eq!(&std::fs::read(destination.join(name)).unwrap(), bytes);
    }

    let undone = fixture.run(&["undo", root]);
    assert_eq!(
        undone.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&undone.stderr)
    );
    for (name, bytes) in &originals {
        assert_eq!(&std::fs::read(fixture.root.join(name)).unwrap(), bytes);
    }
    assert!(
        !destination.exists(),
        "undo left the empty folder sweep created"
    );
}

#[test]
fn a_route_with_no_files_here_is_ignored_and_creates_nothing() {
    let fixture = Fixture::new();
    fixture.profile(&route("xyz", "Absent Folder"));
    fixture.add_files("qoi", 2);
    let root = fixture.root.to_str().unwrap();
    for arguments in [
        vec![root, "--scheme", "existing", "--since", "0"],
        vec![root, "--scheme", "existing", "--since", "0", "--json"],
        vec![
            "apply", root, "--scheme", "existing", "--since", "0", "--yes", "--json",
        ],
    ] {
        let output = fixture.run(&arguments);
        // Nothing to organise is the ordinary exit 1, not a refusal.
        assert_eq!(output.status.code(), Some(1), "{arguments:?}");
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!all.contains("Absent Folder"), "{arguments:?}: {all}");
        assert!(!all.contains("profile_invalid"), "{arguments:?}: {all}");
        assert!(!fixture.root.join("Absent Folder").exists());
    }
    assert_eq!(fixture.source_count("qoi"), 2);
}

#[test]
fn a_route_folder_held_by_a_file_or_a_link_is_refused_by_name() {
    for link in [false, true] {
        let fixture = Fixture::new();
        let holder = fixture.root.join("Box");
        if link {
            std::fs::create_dir(fixture.base.join("elsewhere")).unwrap();
            std::os::unix::fs::symlink(fixture.base.join("elsewhere"), &holder).unwrap();
        } else {
            std::fs::write(&holder, b"synthetic file").unwrap();
        }
        fixture.profile(&route("qoi", "Box"));
        fixture.add_files("qoi", 3);
        let output = fixture.run(&[
            "apply",
            fixture.root.to_str().unwrap(),
            "--scheme",
            "existing",
            "--since",
            "0",
            "--yes",
            "--json",
        ]);
        assert_eq!(output.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&output.stderr);
        let kind = if link { "a link" } else { "a file" };
        let expected = format!(
            "routes .qoi to {}, which is {kind}, not a folder, so nothing moved. \
             Change the .qoi route's folder in that profile.",
            holder.display()
        );
        assert!(stderr.contains(&expected), "{stderr}");
        assert!(!stderr.contains("\"Box\""), "{stderr}");
        assert!(!stderr.contains("Edit that file"), "{stderr}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("\"code\":\"profile_invalid\""),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(fixture.source_count("qoi"), 3);
        if link {
            assert_eq!(
                std::fs::read_dir(fixture.base.join("elsewhere"))
                    .unwrap()
                    .count(),
                0
            );
        } else {
            assert_eq!(std::fs::read(&holder).unwrap(), b"synthetic file");
        }
    }
}

#[test]
fn profile_routed_screenshots_are_not_claimed_again_by_screenshot_grouping() {
    let fixture = Fixture::new();
    let destination = fixture.root.join("Captures");
    std::fs::create_dir(&destination).unwrap();
    fixture.profile(&route("png", "Captures"));
    for index in 1..=3 {
        std::fs::write(
            fixture.root.join(format!("Screenshot {index}.png")),
            b"synthetic screenshot",
        )
        .unwrap();
    }

    let output = fixture.run(&[
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
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fixture.source_count("png"), 0);
    assert_eq!(std::fs::read_dir(destination).unwrap().count(), 3);
    assert!(!fixture.root.join("Screenshots").exists());
}

#[test]
fn depth_two_existing_destination_files_are_reported_in_place() {
    let fixture = Fixture::new();
    let destination = fixture.root.join("Reading");
    std::fs::create_dir(&destination).unwrap();
    fixture.profile(&route("pdf", "Reading"));
    for index in 0..3 {
        std::fs::write(
            destination.join(format!("already-{index}.pdf")),
            b"already in destination",
        )
        .unwrap();
    }

    let output = fixture.run(&[
        fixture.root.to_str().unwrap(),
        "--scheme",
        "existing",
        "--depth",
        "2",
        "--since",
        "0",
        "--json",
    ]);

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("\"name\":\"Reading\""));
    assert!(stdout.contains("\"in_place\":3"), "{stdout}");
    assert_eq!(std::fs::read_dir(destination).unwrap().count(), 3);
}

#[test]
fn depth_three_nested_destination_files_still_promote_to_the_destination() {
    let fixture = Fixture::new();
    let destination = fixture.root.join("Reading");
    let nested = destination.join("sub");
    std::fs::create_dir_all(&nested).unwrap();
    fixture.profile(&route("pdf", "Reading"));
    for index in 0..3 {
        std::fs::write(nested.join(format!("nested-{index}.pdf")), b"nested file").unwrap();
    }

    let output = fixture.run(&[
        fixture.root.to_str().unwrap(),
        "--scheme",
        "existing",
        "--depth",
        "3",
        "--since",
        "0",
        "--json",
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(0));
    assert!(stdout.contains("\"name\":\"Reading\""), "{stdout}");
    assert!(stdout.contains("\"count\":3"), "{stdout}");
}

#[test]
fn two_profile_extensions_refusal_names_the_profile_not_a_map_flag() {
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.root.join("Shared")).unwrap();
    fixture.profile(
        "schema_version = 1\n\
         [[routes]]\nextension = \"qoi\"\nfolder = \"Shared\"\n\
         [[routes]]\nextension = \"webp\"\nfolder = \"Shared\"\n",
    );
    fixture.add_files("qoi", 3);
    fixture.add_files("webp", 3);

    let output = fixture.run(&[
        fixture.root.to_str().unwrap(),
        "--scheme",
        "existing",
        "--since",
        "0",
        "--json",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2));
    assert!(stderr.contains("profile"), "{stderr}");
    assert!(!stderr.contains("--map"), "{stderr}");
    assert_eq!(fixture.source_count("qoi"), 3);
    assert_eq!(fixture.source_count("webp"), 3);
}

#[test]
fn no_journal_json_apply_writes_only_json_to_stdout() {
    let fixture = Fixture::new();
    fixture.add_files("pdf", 3);

    let output = fixture.run(&[
        "apply",
        fixture.root.to_str().unwrap(),
        "--since",
        "0",
        "--yes",
        "--no-journal",
        "--json",
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(0));
    assert!(
        stdout.trim_start().starts_with('{') && stdout.trim_end().ends_with('}'),
        "stdout is not JSON: {stdout}"
    );
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
    // A plan for a missing route folder lists it and moves nothing; only
    // apply creates it.
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"name\":\"Missing\""));
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

#[test]
fn an_exported_plan_creates_its_route_folder_and_refuses_one_made_since() {
    let replayed = Fixture::new();
    replayed.profile(&route("qoi", "Reading"));
    replayed.add_files("qoi", 3);
    let digest = replayed.export();
    assert!(!replayed.root.join("Reading").exists());
    let output = replayed.replay(&digest);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(replayed.source_count("qoi"), 0);
    assert_eq!(
        std::fs::read_dir(replayed.root.join("Reading"))
            .unwrap()
            .count(),
        3
    );

    // A folder that appeared after the plan was made is not the one the plan
    // promised to create, so nothing moves into it.
    let appeared = Fixture::new();
    appeared.profile(&route("qoi", "Reading"));
    appeared.add_files("qoi", 3);
    let digest = appeared.export();
    std::fs::create_dir(appeared.root.join("Reading")).unwrap();
    let output = appeared.replay(&digest);
    assert_ne!(output.status.code(), Some(0));
    assert_eq!(appeared.source_count("qoi"), 3);
    assert_eq!(
        std::fs::read_dir(appeared.root.join("Reading"))
            .unwrap()
            .count(),
        0
    );
}

/// One or two files on a saved route: listed, moved into the folder sweep
/// creates, restored by undo, and the empty folder removed again. A saved
/// route is the person's choice, so the three-file floor does not apply, and
/// the routed files are never reported as having no group.
#[test]
fn one_or_two_files_on_a_saved_route_move_and_undo_removes_the_created_folder() {
    for count in [1usize, 2] {
        let fixture = Fixture::new();
        fixture.profile(&route("pdf", "Docs "));
        let mut originals = Vec::new();
        for index in 0..count {
            let name = format!("synthetic-{index}.pdf");
            let bytes = format!("synthetic pdf payload {index}").into_bytes();
            std::fs::write(fixture.root.join(&name), &bytes).unwrap();
            originals.push((name, bytes));
        }
        let destination = fixture.root.join("Docs ");
        let root = fixture.root.to_str().unwrap();

        let planned = fixture.run(&[root, "--scheme", "existing", "--since", "0"]);
        assert_eq!(planned.status.code(), Some(0));
        let text = String::from_utf8_lossy(&planned.stdout);
        assert!(
            text.contains("sweep creates the folder 'Docs ' for your saved route"),
            "{text}"
        );
        assert!(
            text.contains(".pdf (saved route, new folder sweep creates)"),
            "{text}"
        );
        assert!(!text.contains("matched no group"), "{text}");
        let stderr = String::from_utf8_lossy(&planned.stderr);
        assert!(!stderr.contains("had no group"), "{stderr}");
        assert!(!stderr.contains("sweep propose --help"), "{stderr}");

        let planned = fixture.run(&[root, "--scheme", "existing", "--since", "0", "--json"]);
        assert_eq!(planned.status.code(), Some(0));
        let json = String::from_utf8_lossy(&planned.stdout);
        assert!(json.contains(&format!("\"count\":{count}")), "{json}");
        assert!(json.contains("\"no_clear_group\":0"), "{json}");
        assert!(json.contains("\"no_clear_group_paths\":[]"), "{json}");
        assert!(json.contains("\"unknown_extensions\":{}"), "{json}");
        assert!(!json.contains("routing_hint"), "{json}");
        assert!(!destination.exists(), "planning created the folder");

        let applied = fixture.run(&[
            "apply", root, "--scheme", "existing", "--since", "0", "--yes", "--json",
        ]);
        assert_eq!(
            applied.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&applied.stderr)
        );
        assert_eq!(fixture.source_count("pdf"), 0);
        assert!(!fixture.root.join("Documents").exists());
        for (name, bytes) in &originals {
            assert_eq!(&std::fs::read(destination.join(name)).unwrap(), bytes);
        }

        let undone = fixture.run(&["undo", root]);
        assert_eq!(
            undone.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&undone.stderr)
        );
        for (name, bytes) in &originals {
            assert_eq!(&std::fs::read(fixture.root.join(name)).unwrap(), bytes);
        }
        assert!(!destination.exists(), "undo left the empty created folder");
    }
}

/// The floor guards only folders sweep would make. One or two PDFs move into a
/// Documents folder that is already here, and undo keeps that folder; two
/// PDFs with no Documents folder stay, and the plan says why in true words.
#[test]
fn a_built_in_group_keeps_its_floor_only_when_its_folder_is_missing() {
    for count in [1usize, 2] {
        let fixture = Fixture::new();
        std::fs::create_dir(fixture.root.join("Documents")).unwrap();
        fixture.add_files("pdf", count);
        let root = fixture.root.to_str().unwrap();
        let planned = fixture.run(&[root, "--since", "0"]);
        assert_eq!(planned.status.code(), Some(0));
        let text = String::from_utf8_lossy(&planned.stdout);
        let files = if count == 1 { "1 file " } else { "2 files" };
        assert!(text.contains(&format!("Documents       {files}")), "{text}");
        let applied = fixture.run(&["apply", root, "--since", "0", "--yes"]);
        assert_eq!(
            applied.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&applied.stderr)
        );
        assert_eq!(fixture.source_count("pdf"), 0);
        assert_eq!(
            std::fs::read_dir(fixture.root.join("Documents"))
                .unwrap()
                .count(),
            count
        );
        let undone = fixture.run(&["undo", root]);
        assert_eq!(undone.status.code(), Some(0));
        assert_eq!(fixture.source_count("pdf"), count);
        assert!(
            fixture.root.join("Documents").is_dir(),
            "undo removed a folder sweep did not create"
        );
    }

    let fixture = Fixture::new();
    fixture.add_files("pdf", 2);
    let root = fixture.root.to_str().unwrap();
    let planned = fixture.run(&[root, "--since", "0"]);
    // No group: the plan has nothing to do, which is exit 1, not a failure.
    assert_eq!(planned.status.code(), Some(1));
    let text = String::from_utf8_lossy(&planned.stdout);
    assert!(!text.contains("Documents       2 files"), "{text}");
    assert!(
        text.contains("2 files matched no group and were left where they are"),
        "{text}"
    );
    assert!(
        text.contains(
            "2 files would go to Documents: sweep makes a new folder only for 3 or more files, \
             and moves any number into a Documents folder that is already here"
        ),
        "{text}"
    );
    let applied = fixture.run(&["apply", root, "--since", "0", "--yes"]);
    assert_eq!(fixture.source_count("pdf"), 2, "{applied:?}");
    assert!(!fixture.root.join("Documents").exists());
}

/// The line that says why few files stayed is true for every family name
/// and for a name held by a link: an Images folder, and a Documents link
/// that sweep moves nothing into.
#[test]
fn the_below_floor_line_reads_right_for_a_vowel_name_and_a_link() {
    let fixture = Fixture::new();
    fixture.add_files("png", 2);
    let root = fixture.root.to_str().unwrap();
    let text = String::from_utf8_lossy(&fixture.run(&[root, "--since", "0"]).stdout).into_owned();
    assert!(
        text.contains(
            "2 files would go to Images: sweep makes a new folder only for 3 or more files, \
             and moves any number into an Images folder that is already here"
        ),
        "{text}"
    );
    assert!(!text.contains("a Images"), "{text}");

    let fixture = Fixture::new();
    let elsewhere = fixture.base.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, fixture.root.join("Documents")).unwrap();
    fixture.add_files("pdf", 2);
    let root = fixture.root.to_str().unwrap();
    let text = String::from_utf8_lossy(&fixture.run(&[root, "--since", "0"]).stdout).into_owned();
    assert!(
        text.contains(
            "2 files would go to Documents, but Documents here is a link, not a folder, \
             so sweep moves nothing into it. Use a plain folder named Documents"
        ),
        "{text}"
    );
    assert!(!text.contains("folder that is already here"), "{text}");
    let applied = fixture.run(&["apply", root, "--since", "0", "--yes"]);
    assert_eq!(fixture.source_count("pdf"), 2, "{applied:?}");
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
}
