//! Per-file proposals through the library. Each test names the rule it holds,
//! and each rule's check is the only thing that makes its test pass.

use etude_core::{
    apply::{self, ApplyError},
    plan::{self, BindingContext, BoundPlan},
    proposal::{self, Refusal},
    scan::{self, ScanConfig},
};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

struct Fixture {
    base: PathBuf,
    root: PathBuf,
}

fn context() -> BindingContext {
    BindingContext::new("sweep", "1.0", "scheme-v1", "metadata-contract-v1")
}

impl Fixture {
    fn new() -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!(
            "etudes-proposal-{}-{}-{sequence}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = base.join("tree");
        std::fs::create_dir_all(&root).unwrap();
        // Three screenshots make a group. The rest have no group.
        for i in 1..=3 {
            std::fs::write(
                root.join(format!("Screenshot 2026-01-0{i} at 10.00.00 AM.png")),
                b"synthetic payload",
            )
            .unwrap();
        }
        std::fs::write(root.join("report.pdf"), b"synthetic report").unwrap();
        std::fs::write(root.join("notes.xyz"), b"synthetic notes").unwrap();
        // Everything below must never be eligible.
        std::fs::write(root.join("tax return 2024.pdf"), b"synthetic tax").unwrap();
        std::fs::write(root.join(".hidden.txt"), b"synthetic hidden").unwrap();
        std::os::unix::fs::symlink(root.join("report.pdf"), root.join("link.txt")).unwrap();
        std::fs::write(root.join("big.zip.part"), b"synthetic partial").unwrap();
        std::fs::create_dir(root.join("proj")).unwrap();
        std::fs::write(root.join("proj/Cargo.toml"), b"[package]\n").unwrap();
        std::fs::write(root.join("proj/inner.pdf"), b"synthetic inner").unwrap();
        // Folders a proposal may name.
        std::fs::create_dir(root.join("Reading")).unwrap();
        std::fs::create_dir(root.join("Taxes-2025")).unwrap();
        // Everything so far is old. One file is fresh, so it is too recent.
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
        std::fs::write(root.join("fresh.dat"), b"synthetic fresh").unwrap();
        Self {
            root: root.canonicalize().unwrap(),
            base,
        }
    }

    fn plan_file(&self) -> PathBuf {
        self.base.join("exported.plan")
    }

    fn scan(&self) -> BoundPlan {
        let outcome = scan::scan(
            &self.root,
            &ScanConfig {
                depth: 2,
                grace: Some(Duration::from_secs(86_400)),
                ..Default::default()
            },
        )
        .unwrap();
        let mut proposal = plan::build(&outcome);
        for group in &mut proposal.groups {
            group.accepted = true;
        }
        BoundPlan::from_plan(proposal, context()).unwrap()
    }

    /// Export and load: the plan a caller actually holds.
    fn exported(&self) -> BoundPlan {
        let bound = self.scan();
        let file = self.plan_file();
        let _ = std::fs::remove_file(&file);
        bound.export(&file).unwrap();
        BoundPlan::load(&file, bound.digest()).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn names(plan: &BoundPlan) -> Vec<String> {
    proposal::eligible_items(plan)
        .into_iter()
        .map(|item| item.name)
        .collect()
}

fn id_of(plan: &BoundPlan, name: &str) -> String {
    proposal::eligible_items(plan)
        .into_iter()
        .find(|item| item.name == name)
        .unwrap_or_else(|| panic!("{name} is not eligible"))
        .id
}

fn proposal_json(plan: &BoundPlan, decisions: &str) -> String {
    format!(
        r#"{{"schema_version":1,"plan_digest":"{}","rest":"leave","decisions":[{decisions}]}}"#,
        plan.digest()
    )
}

fn move_to(id: &str, folder: &str) -> String {
    format!(r#"{{"item":"{id}","folder":"{folder}","mode":"require_existing"}}"#)
}

fn skip(id: &str) -> String {
    format!(r#"{{"item":"{id}","skip":true}}"#)
}

fn refused(plan: &BoundPlan, text: &str) -> Refusal {
    proposal::propose(plan, text.as_bytes()).expect_err("this proposal must be refused")
}

fn tree_listing(root: &Path) -> Vec<String> {
    let mut rows = Vec::new();
    fn walk(dir: &Path, root: &Path, rows: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            rows.push(path.strip_prefix(root).unwrap().display().to_string());
            if entry.file_type().unwrap().is_dir() {
                walk(&path, root, rows);
            }
        }
    }
    walk(root, root, &mut rows);
    rows.sort();
    rows
}

// Rule: eligibility comes from sweep's own scan. It is the plan's moves plus
// the files no group claimed, and nothing else.
#[test]
fn eligible_items_are_the_moves_and_the_unclaimed_and_nothing_else() {
    let fixture = Fixture::new();
    let plan = fixture.exported();
    let mut listed = names(&plan);
    listed.sort();
    assert_eq!(
        listed,
        vec![
            "Screenshot 2026-01-01 at 10.00.00 AM.png",
            "Screenshot 2026-01-02 at 10.00.00 AM.png",
            "Screenshot 2026-01-03 at 10.00.00 AM.png",
            "notes.xyz",
            "report.pdf",
        ]
    );
    for never in [
        "tax return 2024.pdf",
        "fresh.dat",
        "big.zip.part",
        "inner.pdf",
        "Cargo.toml",
        "link.txt",
        ".hidden.txt",
    ] {
        assert!(
            !listed.iter().any(|name| name == never),
            "{never} is eligible"
        );
    }
}

// The list carries name, size and the plan's own group, with stable ids.
#[test]
fn items_carry_name_size_group_and_an_index_id() {
    let fixture = Fixture::new();
    let plan = fixture.exported();
    let items = proposal::eligible_items(&plan);
    let ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(
        ids,
        ["i-000000", "i-000001", "i-000002", "i-000003", "i-000004"]
    );
    let report = items.iter().find(|item| item.name == "report.pdf").unwrap();
    assert_eq!(report.size, 16);
    assert_eq!(report.group, None);
    assert_eq!(items[0].group.as_deref(), Some("Screenshots"));
    assert_eq!(
        report.json(),
        r#"{"id":"i-000004","name":"report.pdf","size":16,"group":null}"#
    );
}

// The list a caller sees at export is the list the loaded plan yields.
#[test]
fn the_list_is_frozen_across_export_and_load() {
    let fixture = Fixture::new();
    let live = fixture.scan();
    let loaded = {
        let file = fixture.plan_file();
        live.export(&file).unwrap();
        BoundPlan::load(&file, live.digest()).unwrap()
    };
    assert_eq!(
        proposal::items_json(&proposal::eligible_items(&live)),
        proposal::items_json(&proposal::eligible_items(&loaded))
    );
    assert_eq!(live.digest(), loaded.digest());
}

// Rule: the proposal cites the plan's digest. A different digest is stale.
#[test]
fn a_proposal_citing_another_digest_is_refused_stale() {
    let fixture = Fixture::new();
    let plan = fixture.exported();
    let id = id_of(&plan, "report.pdf");
    let text =
        proposal_json(&plan, &move_to(&id, "Reading")).replace(plan.digest(), &"0".repeat(64));
    assert!(matches!(refused(&plan, &text), Refusal::PlanStale(_)));
}

// Editing the exported plan file is refused by the digest check on load.
#[test]
fn an_edited_plan_file_does_not_load() {
    let fixture = Fixture::new();
    let bound = fixture.scan();
    let file = fixture.plan_file();
    bound.export(&file).unwrap();
    let mut bytes = std::fs::read(&file).unwrap();
    let at = bytes.len() / 2;
    bytes[at] ^= 1;
    std::fs::write(&file, &bytes).unwrap();
    assert!(BoundPlan::load(&file, bound.digest()).is_err());
}

// A forged unclaimed list (a file nobody left alone) is refused on load.
#[test]
fn a_plan_naming_an_unobserved_unclaimed_file_is_refused() {
    let fixture = Fixture::new();
    let mut outcome_plan = {
        let outcome = scan::scan(
            &fixture.root,
            &ScanConfig {
                depth: 2,
                grace: Some(Duration::from_secs(86_400)),
                ..Default::default()
            },
        )
        .unwrap();
        plan::build(&outcome)
    };
    // The personal-looking file is observed, but it is not left alone for want
    // of a group, so it cannot be listed as unclaimed.
    outcome_plan
        .unclaimed
        .push(fixture.root.join("tax return 2024.pdf"));
    assert!(BoundPlan::from_plan(outcome_plan, context()).is_err());
}

// Rule: every item id exists in the plan's eligible list.
#[test]
fn a_guessed_id_for_a_file_that_is_not_eligible_is_unknown_item() {
    let fixture = Fixture::new();
    let plan = fixture.exported();
    let count = proposal::eligible_items(&plan).len();
    // Every id past the end, where a personal-looking file would sit if it were
    // listed, is unknown. So are ids spelled another way.
    for guess in [count, count + 1, count + 6, 999_999] {
        let text = proposal_json(&plan, &move_to(&format!("i-{guess:06}"), "Taxes-2025"));
        assert!(
            matches!(refused(&plan, &text), Refusal::UnknownItem(_)),
            "{guess}"
        );
    }
    for spelled in [
        "i-1",
        "I-000001",
        "item-0",
        "",
        "i-0000001",
        "../etc/passwd",
        "report.pdf",
    ] {
        let text = proposal_json(&plan, &move_to(spelled, "Reading"));
        assert!(
            matches!(refused(&plan, &text), Refusal::UnknownItem(_)),
            "{spelled:?}"
        );
    }
}

// Rule: an item is named at most once, even if both decisions agree.
#[test]
fn an_item_named_twice_is_refused_even_when_the_decisions_agree() {
    let fixture = Fixture::new();
    let plan = fixture.exported();
    let id = id_of(&plan, "report.pdf");
    for decisions in [
        format!("{},{}", move_to(&id, "Reading"), move_to(&id, "Reading")),
        format!("{},{}", move_to(&id, "Reading"), move_to(&id, "Taxes-2025")),
        format!("{},{}", move_to(&id, "Reading"), skip(&id)),
        format!("{},{}", skip(&id), skip(&id)),
    ] {
        assert!(
            matches!(
                refused(&plan, &proposal_json(&plan, &decisions)),
                Refusal::ItemAssignedTwice(_)
            ),
            "{decisions}"
        );
    }
}

// Rule: a folder is one plain name.
#[test]
fn a_folder_is_one_plain_name() {
    let fixture = Fixture::new();
    let plan = fixture.exported();
    let id = id_of(&plan, "report.pdf");
    for folder in [
        "",
        ".",
        "..",
        "../out",
        "Reading/sub",
        "/tmp",
        "a\\\\b",
        ".hidden",
        "../tree",
    ] {
        let text = proposal_json(&plan, &move_to(&id, folder));
        assert!(
            matches!(refused(&plan, &text), Refusal::DestinationOutsideRoot(_)),
            "{folder:?}"
        );
    }
}

// Rule: require_existing needs the folder now, and a name alone never creates one.
#[test]
fn a_missing_folder_is_refused_and_never_created() {
    let fixture = Fixture::new();
    let plan = fixture.exported();
    let id = id_of(&plan, "report.pdf");
    let before = tree_listing(&fixture.root);
    let err = refused(&plan, &proposal_json(&plan, &move_to(&id, "Absent")));
    assert!(matches!(err, Refusal::ProfileMissing(_)), "{err}");
    assert_eq!(tree_listing(&fixture.root), before);
}

// Rule: a non-folder holding the name refuses, and a link is not a folder.
#[test]
fn a_file_or_a_link_holding_the_folder_name_is_refused() {
    let fixture = Fixture::new();
    std::fs::write(fixture.root.join("Occupied"), b"synthetic file").unwrap();
    std::os::unix::fs::symlink(fixture.root.join("Reading"), fixture.root.join("Linked")).unwrap();
    let plan = fixture.exported();
    let id = id_of(&plan, "report.pdf");
    for folder in ["Occupied", "Linked", "link.txt"] {
        let err = refused(&plan, &proposal_json(&plan, &move_to(&id, folder)));
        assert!(
            matches!(err, Refusal::DestinationExists(_)),
            "{folder}: {err}"
        );
    }
}

// Rule: a folder made after the export is not the folder the plan observed.
#[test]
fn a_folder_created_after_the_export_is_stale() {
    let fixture = Fixture::new();
    let plan = fixture.exported();
    std::fs::create_dir(fixture.root.join("Later")).unwrap();
    let id = id_of(&plan, "report.pdf");
    let err = refused(&plan, &proposal_json(&plan, &move_to(&id, "Later")));
    assert!(matches!(err, Refusal::PlanStale(_)), "{err}");
}

// Rule: a file keeps its name, and a collision at the destination refuses.
#[test]
fn a_name_already_taken_at_the_destination_is_refused() {
    let fixture = Fixture::new();
    std::fs::write(fixture.root.join("Reading/report.pdf"), b"synthetic other").unwrap();
    let plan = fixture.exported();
    let id = id_of(&plan, "report.pdf");
    let err = refused(&plan, &proposal_json(&plan, &move_to(&id, "Reading")));
    assert!(matches!(err, Refusal::DestinationExists(_)), "{err}");
}

#[test]
fn two_items_with_one_name_cannot_share_a_folder() {
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.root.join("a")).unwrap();
    std::fs::create_dir(fixture.root.join("b")).unwrap();
    std::fs::write(fixture.root.join("a/same.xyz"), b"synthetic one").unwrap();
    std::fs::write(fixture.root.join("b/same.xyz"), b"synthetic two").unwrap();
    let old = SystemTime::now() - Duration::from_secs(10 * 86_400);
    for folder in ["a", "b"] {
        std::fs::File::options()
            .write(true)
            .open(fixture.root.join(folder).join("same.xyz"))
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
    let plan = fixture.exported();
    let same: Vec<String> = proposal::eligible_items(&plan)
        .into_iter()
        .filter(|item| item.name == "same.xyz")
        .map(|item| item.id)
        .collect();
    assert_eq!(same.len(), 2);
    let decisions = format!(
        "{},{}",
        move_to(&same[0], "Reading"),
        move_to(&same[1], "Reading")
    );
    let err = refused(&plan, &proposal_json(&plan, &decisions));
    assert!(matches!(err, Refusal::DestinationExists(_)), "{err}");
    // The same two files go to different folders without trouble.
    let decisions = format!(
        "{},{}",
        move_to(&same[0], "Reading"),
        move_to(&same[1], "Taxes-2025")
    );
    assert!(proposal::propose(&plan, proposal_json(&plan, &decisions).as_bytes()).is_ok());
}

// Rules: `rest` is required and is "leave"; size, count and JSON bounds hold;
// version 1 has no create_if_absent.
#[test]
fn rest_and_the_document_shape_are_checked() {
    let fixture = Fixture::new();
    let plan = fixture.exported();
    let id = id_of(&plan, "report.pdf");
    let digest = plan.digest();
    let decision = move_to(&id, "Reading");
    let cases = [
        // rest missing, wrong, or not a string
        format!(r#"{{"schema_version":1,"plan_digest":"{digest}","decisions":[{decision}]}}"#),
        format!(
            r#"{{"schema_version":1,"plan_digest":"{digest}","rest":"keep_plan","decisions":[{decision}]}}"#
        ),
        format!(
            r#"{{"schema_version":1,"plan_digest":"{digest}","rest":"Leave","decisions":[{decision}]}}"#
        ),
        format!(
            r#"{{"schema_version":1,"plan_digest":"{digest}","rest":null,"decisions":[{decision}]}}"#
        ),
        // version
        format!(
            r#"{{"schema_version":2,"plan_digest":"{digest}","rest":"leave","decisions":[{decision}]}}"#
        ),
        format!(r#"{{"plan_digest":"{digest}","rest":"leave","decisions":[{decision}]}}"#),
        // no create_if_absent, no missing or other mode
        format!(
            r#"{{"schema_version":1,"plan_digest":"{digest}","rest":"leave","decisions":[{{"item":"{id}","folder":"Reading","mode":"create_if_absent"}}]}}"#
        ),
        format!(
            r#"{{"schema_version":1,"plan_digest":"{digest}","rest":"leave","decisions":[{{"item":"{id}","folder":"Reading"}}]}}"#
        ),
        // skip must be true and stand alone
        format!(
            r#"{{"schema_version":1,"plan_digest":"{digest}","rest":"leave","decisions":[{{"item":"{id}","skip":false}}]}}"#
        ),
        format!(
            r#"{{"schema_version":1,"plan_digest":"{digest}","rest":"leave","decisions":[{{"item":"{id}","skip":true,"folder":"Reading","mode":"require_existing"}}]}}"#
        ),
        // unknown fields, duplicate keys, not JSON
        format!(
            r#"{{"schema_version":1,"plan_digest":"{digest}","rest":"leave","decisions":[{decision}],"extra":1}}"#
        ),
        format!(
            r#"{{"schema_version":1,"schema_version":1,"plan_digest":"{digest}","rest":"leave","decisions":[{decision}]}}"#
        ),
        "not json".to_string(),
        "[]".to_string(),
        String::new(),
        format!(
            r#"{{"schema_version":1,"plan_digest":"{digest}","rest":"leave","decisions":[{decision}]}} trailing"#
        ),
    ];
    for text in cases {
        assert!(
            matches!(refused(&plan, &text), Refusal::InvalidArguments(_)),
            "{text}"
        );
    }
}

#[test]
fn size_and_count_limits_hold() {
    let fixture = Fixture::new();
    let plan = fixture.exported();
    let id = id_of(&plan, "report.pdf");
    // 5,000 decisions are allowed by count. The same item 5,001 times is over
    // the limit before anything else is looked at.
    let many = vec![skip(&id); 5_001].join(",");
    assert!(matches!(
        refused(&plan, &proposal_json(&plan, &many)),
        Refusal::InvalidArguments(m) if m.contains("5,000")
    ));
    let at_limit = vec![skip(&id); 5_000].join(",");
    assert!(matches!(
        refused(&plan, &proposal_json(&plan, &at_limit)),
        Refusal::ItemAssignedTwice(_)
    ));
    // Over 1 MiB, however well formed.
    let padding = " ".repeat(1024 * 1024);
    let big = format!(
        "{}{padding}",
        proposal_json(&plan, &move_to(&id, "Reading"))
    );
    assert!(matches!(
        refused(&plan, &big),
        Refusal::InvalidArguments(m) if m.contains("1 MiB")
    ));
    // Exactly 1 MiB is accepted.
    let ok = proposal_json(&plan, &move_to(&id, "Reading"));
    let exact = format!("{ok}{}", " ".repeat(1024 * 1024 - ok.len()));
    assert_eq!(exact.len(), 1024 * 1024);
    assert!(proposal::propose(&plan, exact.as_bytes()).is_ok());
}

// Rule: refusals are all-or-nothing. One bad decision among good ones refuses
// the proposal; the plan and the tree are untouched.
#[test]
fn one_bad_decision_refuses_the_whole_proposal() {
    let fixture = Fixture::new();
    let plan = fixture.exported();
    let before = tree_listing(&fixture.root);
    let good = move_to(&id_of(&plan, "report.pdf"), "Reading");
    let good2 = move_to(&id_of(&plan, "notes.xyz"), "Taxes-2025");
    let bad = move_to("i-000099", "Reading");
    for decisions in [
        format!("{good},{bad},{good2}"),
        format!("{bad},{good},{good2}"),
        format!("{good},{good2},{bad}"),
    ] {
        assert!(proposal::propose(&plan, proposal_json(&plan, &decisions).as_bytes()).is_err());
    }
    assert_eq!(tree_listing(&fixture.root), before);
    let ok = format!("{good},{good2}");
    assert!(proposal::propose(&plan, proposal_json(&plan, &ok).as_bytes()).is_ok());
}

// Only sweep plans take proposals.
#[test]
fn a_plan_from_another_tool_takes_no_proposal() {
    let fixture = Fixture::new();
    let outcome = scan::scan(&fixture.root, &ScanConfig::default()).unwrap();
    let mut other = plan::build(&outcome);
    for group in &mut other.groups {
        group.accepted = true;
    }
    let plan = BoundPlan::from_plan(other, BindingContext::new("stash", "1.0", "p", "c")).unwrap();
    assert!(matches!(
        refused(&plan, &proposal_json(&plan, "")),
        Refusal::InvalidArguments(_)
    ));
}

// A skipped item, and every item not named, stays where it is. Only the named
// items move, each keeping its name, and the plan applies like any other.
#[test]
fn a_proposal_moves_only_what_it_names_and_keeps_names() {
    let fixture = Fixture::new();
    let plan = fixture.exported();
    let report = id_of(&plan, "report.pdf");
    let notes = id_of(&plan, "notes.xyz");
    let shot = id_of(&plan, "Screenshot 2026-01-01 at 10.00.00 AM.png");
    let decisions = format!(
        "{},{},{}",
        move_to(&report, "Reading"),
        skip(&notes),
        move_to(&shot, "Taxes-2025")
    );
    let proposed = proposal::propose(&plan, proposal_json(&plan, &decisions).as_bytes()).unwrap();
    assert_eq!((proposed.moves, proposed.skipped), (2, 1));
    let before = tree_listing(&fixture.root);
    let mut bound = proposed.plan;
    for group in &mut bound.plan.groups {
        group.accepted = true;
    }
    bound.finalize_choices().unwrap();
    apply::apply(&bound, &context(), None, None).unwrap();
    let after = tree_listing(&fixture.root);
    assert!(fixture.root.join("Reading/report.pdf").is_file());
    assert!(
        fixture
            .root
            .join("Taxes-2025/Screenshot 2026-01-01 at 10.00.00 AM.png")
            .is_file()
    );
    // Skipped, unnamed and ineligible files are where they were.
    for stays in [
        "notes.xyz",
        "Screenshot 2026-01-02 at 10.00.00 AM.png",
        "Screenshot 2026-01-03 at 10.00.00 AM.png",
        "tax return 2024.pdf",
        "fresh.dat",
    ] {
        assert!(fixture.root.join(stays).exists(), "{stays} moved");
    }
    assert_eq!(before.len(), after.len());
}

// The same plan and proposal give the same new plan.
#[test]
fn the_same_proposal_gives_the_same_digest() {
    let fixture = Fixture::new();
    let plan = fixture.exported();
    let text = proposal_json(&plan, &move_to(&id_of(&plan, "report.pdf"), "Reading"));
    let first = proposal::propose(&plan, text.as_bytes()).unwrap();
    let second = proposal::propose(&plan, text.as_bytes()).unwrap();
    assert_eq!(first.plan.digest(), second.plan.digest());
    assert_ne!(first.plan.digest(), plan.digest());
}

// The new plan is exported and loaded like any other, with the same list rules.
#[test]
fn the_new_plan_round_trips_and_keeps_unmoved_files_eligible() {
    let fixture = Fixture::new();
    let plan = fixture.exported();
    let text = proposal_json(&plan, &move_to(&id_of(&plan, "report.pdf"), "Reading"));
    let proposed = proposal::propose(&plan, text.as_bytes()).unwrap();
    let out = fixture.base.join("out.plan");
    proposed.plan.export(&out).unwrap();
    let loaded = BoundPlan::load(&out, proposed.plan.digest()).unwrap();
    assert_eq!(loaded.digest(), proposed.plan.digest());
    let mut listed = names(&loaded);
    listed.sort();
    assert!(listed.contains(&"report.pdf".to_string()));
    assert!(listed.contains(&"notes.xyz".to_string()));
    assert_eq!(listed.len(), 5);
    assert!(!listed.iter().any(|name| name == "tax return 2024.pdf"));
}

// A destination swapped for a link between propose and apply is refused at
// apply, and nothing moves.
#[test]
fn a_folder_swapped_for_a_link_after_propose_is_refused_at_apply() {
    let fixture = Fixture::new();
    let plan = fixture.exported();
    let outside = fixture.base.join("elsewhere");
    std::fs::create_dir(&outside).unwrap();
    let text = proposal_json(&plan, &move_to(&id_of(&plan, "report.pdf"), "Reading"));
    let proposed = proposal::propose(&plan, text.as_bytes()).unwrap();
    let out = fixture.base.join("out.plan");
    proposed.plan.export(&out).unwrap();
    let mut loaded = BoundPlan::load(&out, proposed.plan.digest()).unwrap();
    std::fs::remove_dir(fixture.root.join("Reading")).unwrap();
    std::os::unix::fs::symlink(&outside, fixture.root.join("Reading")).unwrap();
    for group in &mut loaded.plan.groups {
        group.accepted = true;
    }
    let err = apply::apply(&loaded, &context(), None, None).unwrap_err();
    assert!(
        matches!(
            err,
            ApplyError::StalePlan(_) | ApplyError::DestinationDirectoryChanged(_)
        ),
        "{err}"
    );
    assert!(fixture.root.join("report.pdf").is_file());
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

// The destination folder is bound by identity inside the plan itself. Applying
// the plan's groups without the whole-tree check still refuses a swapped
// folder, with the executor's own named refusal.
#[test]
fn the_executors_identity_check_refuses_a_swapped_folder_on_its_own() {
    let fixture = Fixture::new();
    let plan = fixture.exported();
    // A link that stays inside the root, so only the identity check can object.
    let inside = fixture.root.join("Taxes-2025");
    let text = proposal_json(&plan, &move_to(&id_of(&plan, "report.pdf"), "Reading"));
    let proposed = proposal::propose(&plan, text.as_bytes()).unwrap();
    let out = fixture.base.join("out.plan");
    proposed.plan.export(&out).unwrap();
    let loaded = BoundPlan::load(&out, proposed.plan.digest()).unwrap();
    let mut unbound = loaded.plan.clone();
    for group in &mut unbound.groups {
        group.accepted = true;
    }
    std::fs::remove_dir(fixture.root.join("Reading")).unwrap();
    std::os::unix::fs::symlink(&inside, fixture.root.join("Reading")).unwrap();
    let err =
        apply::apply_with_structured_progress(&unbound, "sweep", None, None, |_| {}).unwrap_err();
    assert!(
        matches!(err, ApplyError::DestinationDirectoryChanged(_)),
        "{err}"
    );
    assert!(fixture.root.join("report.pdf").is_file());
    assert_eq!(std::fs::read_dir(&inside).unwrap().count(), 0);
}
