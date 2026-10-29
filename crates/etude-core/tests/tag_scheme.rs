//! `--scheme tags` in the library: a tag plan adds Finder tags and moves
//! nothing, only adds, and undo puts each file's tag attribute back byte for
//! byte.
//!
//! Tag values are written and read here through the system's own tools
//! (`xattr` and `plutil`), not through the code under test, so a bug that
//! encodes and decodes symmetrically would still show.
#![cfg(target_os = "macos")]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};

use etude_core::apply::{self, ApplyFaults, TagRefusal};
use etude_core::journal::{EntryState, Journal, Sealer};
use etude_core::plan::{self, BindingContext, BoundPlan, Plan, TagSpec};
use etude_core::scan::{self, ScanConfig};
use etude_core::{Signal, Untouched};

const TAG: &str = "com.apple.metadata:_kMDItemUserTags";
const COMMENT: &str = "com.apple.metadata:kMDItemFinderComment";

fn lock() -> MutexGuard<'static, ()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

struct TestSeal;
impl Sealer for TestSeal {
    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
        let mut v = b"TESTSEAL".to_vec();
        v.extend(plaintext.iter().map(|b| b ^ 0x5a));
        Ok(v)
    }
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
        let body = sealed
            .strip_prefix(b"TESTSEAL".as_slice())
            .ok_or("bad header")?;
        Ok(body.iter().map(|b| b ^ 0x5a).collect())
    }
}

/// Fails to seal exactly the progress frames that start with `prefix`.
struct FailingSeal {
    prefix: &'static [u8],
}
impl Sealer for FailingSeal {
    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, &'static str> {
        if plaintext.starts_with(self.prefix) {
            return Err("injected journal write failure");
        }
        TestSeal.seal(plaintext)
    }
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
        TestSeal.open(sealed)
    }
}

struct Scene {
    base: PathBuf,
    root: PathBuf,
}

impl Drop for Scene {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn scene(label: &str, names: &[&str]) -> Scene {
    let base = std::env::temp_dir().join(format!("etude_tagscheme_{label}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let root = base.join("root");
    fs::create_dir_all(&root).unwrap();
    unsafe { std::env::set_var("ETUDE_STATE_DIR", base.join("state")) };
    let root = root.canonicalize().unwrap();
    for name in names {
        fs::write(root.join(name), format!("contents of {name}")).unwrap();
    }
    Scene { base, root }
}

impl Scene {
    fn file(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

// --- the system's own view of a tag attribute --------------------------------

fn hex_to_bytes(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text.bytes().filter(u8::is_ascii_hexdigit).collect();
    digits
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn raw(path: &Path) -> Option<Vec<u8>> {
    let out = Command::new("xattr")
        .args(["-px", TAG])
        .arg(path)
        .output()
        .unwrap();
    out.status
        .success()
        .then(|| hex_to_bytes(&String::from_utf8_lossy(&out.stdout)))
}

fn set_raw(path: &Path, bytes: &[u8]) {
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    assert!(
        Command::new("xattr")
            .args(["-wx", TAG, &hex])
            .arg(path)
            .status()
            .unwrap()
            .success()
    );
}

fn json_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\n', "\\n"))
}

/// A tag list as Finder would store it, built by the system's plutil.
fn plist_of(tags: &[&str]) -> Vec<u8> {
    let json = format!(
        "[{}]",
        tags.iter()
            .map(|t| json_string(t))
            .collect::<Vec<_>>()
            .join(",")
    );
    let mut child = Command::new("plutil")
        .args(["-convert", "binary1", "-o", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(json.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    out.stdout
}

fn set_tags(path: &Path, tags: &[&str]) {
    set_raw(path, &plist_of(tags));
}

/// The tags a file carries according to the system's plutil.
fn tags_of(path: &Path) -> Option<Vec<String>> {
    let bytes = raw(path)?;
    let mut child = Command::new("plutil")
        .args(["-convert", "json", "-o", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&bytes).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "the system could not parse the value");
    let text = String::from_utf8(out.stdout).unwrap();
    let inner = text.trim().trim_start_matches('[').trim_end_matches(']');
    let mut tags = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match (in_string, c) {
            (false, '"') => in_string = true,
            (true, '"') => {
                tags.push(std::mem::take(&mut current));
                in_string = false;
            }
            (true, '\\') => match chars.next() {
                Some('n') => current.push('\n'),
                Some(other) => current.push(other),
                None => {}
            },
            (true, c) => current.push(c),
            _ => {}
        }
    }
    Some(tags)
}

/// Every file's tag bytes, mtime and ctime, so "nothing was touched" is
/// checked against the file system and not against the tool's own counters.
type Row = (String, Option<Vec<u8>>, i64, i64, i64);

fn fingerprint_tree(root: &Path) -> Vec<Row> {
    use std::os::unix::fs::MetadataExt;
    let mut rows: Vec<_> = fs::read_dir(root)
        .unwrap()
        .flatten()
        .map(|e| {
            let m = fs::symlink_metadata(e.path()).unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                raw(&e.path()),
                m.mtime_nsec() + m.mtime() * 1_000_000_000,
                m.ctime_nsec() + m.ctime() * 1_000_000_000,
                m.ino() as i64,
            )
        })
        .collect();
    rows.sort();
    rows
}

// --- planning and applying ------------------------------------------------------

fn spec(ext: &str, name: &str) -> TagSpec {
    TagSpec {
        ext: ext.into(),
        name: name.into(),
    }
}

fn scan_config() -> ScanConfig {
    ScanConfig {
        grace: None,
        include_tagged: true,
        ..ScanConfig::default()
    }
}

fn plan_tags(s: &Scene, specs: &[TagSpec]) -> Plan {
    let outcome = scan::scan(&s.root, &scan_config()).expect("scan");
    let mut plan = plan::build_tags(&outcome, specs, None);
    for group in &mut plan.groups {
        group.accepted = true;
    }
    plan
}

fn apply_plan(plan: &Plan) -> apply::ApplyReport {
    apply::apply_with_structured_progress(plan, "test", Some(&TestSeal), None, |_| {})
        .expect("apply")
}

fn reload(report: &apply::ApplyReport) -> Journal {
    Journal::load_sealed("test", &report.journal_id, &TestSeal).expect("journal reloads")
}

// --- the rules ----------------------------------------------------------------

#[test]
fn adding_a_tag_keeps_every_existing_tag_in_their_order() {
    let _g = lock();
    let s = scene("addonly", &["a.pdf", "b.pdf", "c.pdf"]);
    set_tags(&s.file("a.pdf"), &["Red\n6", "Old", "Zeta"]);
    set_tags(&s.file("b.pdf"), &["Caf\u{e9}"]);
    // c.pdf has no tag attribute at all.
    let report = apply_plan(&plan_tags(&s, &[spec("pdf", "Work")]));
    assert_eq!(report.tagged, 3);
    assert_eq!(
        tags_of(&s.file("a.pdf")).unwrap(),
        ["Red\n6", "Old", "Zeta", "Work"]
    );
    assert_eq!(tags_of(&s.file("b.pdf")).unwrap(), ["Caf\u{e9}", "Work"]);
    assert_eq!(tags_of(&s.file("c.pdf")).unwrap(), ["Work"]);
}

#[test]
fn a_file_that_already_has_the_tag_is_left_exactly_as_it_is() {
    let _g = lock();
    let s = scene("present", &["a.pdf", "b.pdf"]);
    set_tags(&s.file("a.pdf"), &["work\n2", "Old"]);
    let before = raw(&s.file("a.pdf"));
    let plan = plan_tags(&s, &[spec("pdf", "Work")]);
    assert_eq!(plan.tag_already_present(), 1);
    let report = apply_plan(&plan);
    assert_eq!(report.tagged, 1);
    assert_eq!(
        raw(&s.file("a.pdf")),
        before,
        "not rewritten, not duplicated"
    );
}

#[test]
fn a_tag_that_is_on_a_file_in_the_folder_is_not_new_and_keeps_its_colour() {
    let _g = lock();
    let s = scene("known", &["a.pdf", "b.png", "c.png"]);
    set_tags(&s.file("b.png"), &["Receipts\n4"]);
    // Asked for in different case: it is the same tag, spelled as it exists.
    let plan = plan_tags(&s, &[spec("pdf", "receipts"), spec("png", "Trips")]);
    let rows = plan.tag_summary();
    let receipts = rows.iter().find(|r| r.name == "Receipts").expect("row");
    assert!(!receipts.new);
    let trips = rows.iter().find(|r| r.name == "Trips").expect("row");
    assert!(trips.new);
    let entry = plan
        .groups
        .iter()
        .find_map(|g| match &g.signal {
            Signal::Tagged { ext, entry, .. } if ext == "pdf" => Some(entry.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(entry, "Receipts\n4");
    apply_plan(&plan);
    assert_eq!(tags_of(&s.file("a.pdf")).unwrap(), ["Receipts\n4"]);
    assert_eq!(tags_of(&s.file("b.png")).unwrap(), ["Receipts\n4", "Trips"]);
}

#[test]
fn a_tag_on_no_file_is_marked_new_in_the_plan_and_the_json() {
    let _g = lock();
    let s = scene("new", &["a.pdf", "b.png"]);
    set_tags(&s.file("b.png"), &["Old"]);
    let plan = plan_tags(&s, &[spec("pdf", "Brand New")]);
    assert!(plan.tag_summary()[0].new);
    let json = plan.to_json();
    assert!(json.contains("\"new_tags\":[\"Brand New\"]"), "{json}");
    assert!(
        json.contains("\"name\":\"Brand New\",\"files\":1,\"new\":true"),
        "{json}"
    );
    assert!(json.contains("\"new_tag_basis\""), "{json}");
}

#[test]
fn a_colour_tag_finder_ships_with_is_written_with_its_colour_number() {
    let _g = lock();
    let s = scene("colour", &["a.pdf"]);
    let plan = plan_tags(&s, &[spec("pdf", "orange")]);
    apply_plan(&plan);
    assert_eq!(tags_of(&s.file("a.pdf")).unwrap(), ["Orange\n7"]);
}

#[test]
fn a_tagged_file_is_tagged_without_include_tagged() {
    let _g = lock();
    let s = scene("tagged", &["a.pdf"]);
    set_tags(&s.file("a.pdf"), &["Keep"]);
    let plan = plan_tags(&s, &[spec("pdf", "Work")]);
    assert!(
        !plan.include_tagged,
        "the plan does not claim include-tagged"
    );
    assert_eq!(plan.skipped_tagged, 0);
    apply_plan(&plan);
    assert_eq!(tags_of(&s.file("a.pdf")).unwrap(), ["Keep", "Work"]);
}

#[test]
fn planning_writes_no_tag_and_touches_no_file_metadata() {
    let _g = lock();
    let s = scene("planonly", &["a.pdf", "b.pdf", "c.png"]);
    set_tags(&s.file("a.pdf"), &["Old"]);
    let before = fingerprint_tree(&s.root);
    let outcome = scan::scan(&s.root, &scan_config()).expect("scan");
    let plan = plan::build_tags(&outcome, &[spec("pdf", "Work"), spec("png", "Pics")], None);
    let _ = plan.to_json();
    assert_eq!(
        plan.moves(),
        0,
        "nothing is accepted until a person applies"
    );
    assert_eq!(
        fingerprint_tree(&s.root),
        before,
        "tag bytes, mtime, ctime and inode of every file are unchanged"
    );
}

#[test]
fn applying_moves_nothing_and_creates_nothing() {
    let _g = lock();
    let s = scene("nomove", &["a.pdf", "b.pdf"]);
    let names_before = {
        let mut v: Vec<_> = fs::read_dir(&s.root)
            .unwrap()
            .flatten()
            .map(|e| (e.file_name(), fs::read(e.path()).unwrap()))
            .collect();
        v.sort();
        v
    };
    let report = apply_plan(&plan_tags(&s, &[spec("pdf", "Work")]));
    assert_eq!((report.tagged, report.moved), (2, 0));
    let mut names_after: Vec<_> = fs::read_dir(&s.root)
        .unwrap()
        .flatten()
        .map(|e| (e.file_name(), fs::read(e.path()).unwrap()))
        .collect();
    names_after.sort();
    assert_eq!(names_after, names_before, "same names, same contents");
}

#[test]
fn undo_puts_back_the_exact_previous_attribute() {
    let _g = lock();
    let s = scene("undo", &["a.pdf", "b.pdf", "c.pdf"]);
    set_tags(&s.file("a.pdf"), &["Red\n6", "Old"]);
    // A value another program wrote with its objects in a different order: a
    // restore that re-encoded the list would not give these bytes back.
    let odd = {
        let mut v = b"bplist00".to_vec();
        v.extend([0xa2, 0x02, 0x01]); // array: refs to objects 2 then 1
        v.extend([0x51, b'B']); // object 1 at offset 11
        v.extend([0x51, b'A']); // object 2 at offset 13
        v.extend([8, 11, 13]); // offset table at 15
        v.extend([0, 0, 0, 0, 0, 0, 1, 1]);
        v.extend(3u64.to_be_bytes());
        v.extend(0u64.to_be_bytes());
        v.extend(15u64.to_be_bytes());
        v
    };
    set_raw(&s.file("b.pdf"), &odd);
    let before_a = raw(&s.file("a.pdf"));
    assert_eq!(tags_of(&s.file("b.pdf")).unwrap(), ["A", "B"]);
    let report = apply_plan(&plan_tags(&s, &[spec("pdf", "Work")]));
    assert_eq!(report.tagged, 3);
    assert_ne!(raw(&s.file("a.pdf")), before_a);

    let mut journal = reload(&report);
    let undone = apply::undo(&mut journal, &TestSeal);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.tags_restored, 3);
    assert!(undone.tags_refused.is_empty());
    assert_eq!(raw(&s.file("a.pdf")), before_a, "exact bytes");
    assert_eq!(raw(&s.file("b.pdf")).as_deref(), Some(odd.as_slice()));
    assert_eq!(
        raw(&s.file("c.pdf")),
        None,
        "a file with no attribute has none again"
    );
    assert!(
        !apply::has_undo_work(&journal, &TestSeal),
        "nothing is left to undo"
    );
}

#[test]
fn undo_refuses_a_file_whose_tags_changed_since_apply_and_names_why() {
    let _g = lock();
    let s = scene("changed", &["a.pdf", "b.pdf", "c.pdf"]);
    set_tags(&s.file("a.pdf"), &["Old"]);
    let report = apply_plan(&plan_tags(&s, &[spec("pdf", "Work")]));
    // Someone adds a tag to b.pdf after apply.
    set_tags(&s.file("b.pdf"), &["Work", "Added Later"]);
    let mut journal = reload(&report);
    let undone = apply::undo(&mut journal, &TestSeal);
    assert_eq!(undone.tags_restored, 2);
    assert_eq!(
        undone.tags_refused,
        vec![(s.file("b.pdf"), TagRefusal::Changed)]
    );
    assert_eq!(
        tags_of(&s.file("b.pdf")).unwrap(),
        ["Work", "Added Later"],
        "their change was not overwritten"
    );
    assert_eq!(tags_of(&s.file("a.pdf")).unwrap(), ["Old"]);
    // Running undo again repeats the refusal and still overwrites nothing.
    let again = apply::undo(&mut journal, &TestSeal);
    assert_eq!(again.tags_restored, 0);
    assert_eq!(again.tags_refused.len(), 1);
    assert_eq!(tags_of(&s.file("b.pdf")).unwrap(), ["Work", "Added Later"]);
}

#[test]
fn an_apply_stopped_part_way_is_undone_from_the_journal() {
    let _g = lock();
    let s = scene("partial", &["a.pdf", "b.pdf", "c.pdf", "d.pdf"]);
    set_tags(&s.file("a.pdf"), &["Old"]);
    let before: Vec<_> = ["a.pdf", "b.pdf", "c.pdf", "d.pdf"]
        .iter()
        .map(|n| raw(&s.file(n)))
        .collect();
    let plan = plan_tags(&s, &[spec("pdf", "Work")]);
    let error =
        apply::apply_with_structured_progress(&plan, "test", Some(&TestSeal), Some(2), |_| {})
            .expect_err("stopped at entry 2");
    assert!(matches!(error, apply::ApplyError::Injected(2)));
    // Find the journal the stopped apply left.
    let state = s.base.join("state");
    let id = fs::read_dir(&state)
        .unwrap()
        .flatten()
        .find_map(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_prefix("test-")
                .and_then(|n| n.strip_suffix(".journal"))
                .map(str::to_string)
        })
        .expect("a journal exists");
    let mut journal = Journal::load_sealed("test", &id, &TestSeal).unwrap();
    let states: Vec<_> = journal.entries.iter().map(|e| e.state).collect();
    assert_eq!(
        states,
        [
            EntryState::Moved,
            EntryState::Moved,
            EntryState::Planned,
            EntryState::Planned
        ],
        "a real partial state: two written, two not"
    );
    let undone = apply::undo(&mut journal, &TestSeal);
    assert!(undone.error.is_none());
    assert_eq!(undone.tags_restored, 2);
    let after: Vec<_> = ["a.pdf", "b.pdf", "c.pdf", "d.pdf"]
        .iter()
        .map(|n| raw(&s.file(n)))
        .collect();
    assert_eq!(after, before);
}

#[test]
fn a_tag_written_without_a_done_record_is_retained_without_guessing() {
    let _g = lock();
    let s = scene("unrecorded", &["a.pdf", "b.pdf"]);
    set_tags(&s.file("a.pdf"), &["Old"]);
    let plan = plan_tags(&s, &[spec("pdf", "Work")]);
    // The write lands, then the record of it cannot be written.
    let failure = apply::apply_with_structured_progress_outcome(
        &plan,
        "test",
        Some(&FailingSeal { prefix: b"done" }),
        None,
        |_| {},
    )
    .expect_err("the done record fails");
    let id = failure.durable_journal_id.expect("journal id");
    let mut journal = Journal::load_sealed("test", &id, &TestSeal).unwrap();
    assert_eq!(journal.entries[0].state, EntryState::Planned);
    assert_eq!(apply::unrecorded_moves(&journal), 0);
    assert!(apply::has_undo_work(&journal, &TestSeal));
    let written = raw(&s.file("a.pdf"));
    assert!(written.is_some());
    let undone = apply::undo(&mut journal, &TestSeal);
    assert!(undone.error.is_none());
    assert_eq!(undone.tags_restored, 0);
    assert_eq!(undone.skipped_changed.len(), 2);
    assert_eq!(raw(&s.file("a.pdf")), written);
    assert_eq!(raw(&s.file("b.pdf")), None);
    let mut journal = Journal::load_sealed("test", &id, &TestSeal).unwrap();
    assert!(!apply::has_undo_work(&journal, &TestSeal));
    let again = apply::undo(&mut journal, &TestSeal);
    assert!(again.skipped_changed.is_empty());
    assert_eq!(raw(&s.file("a.pdf")), written);
}

#[test]
fn a_file_with_no_readable_tag_state_is_skipped_with_a_reason_and_never_guessed() {
    let _g = lock();
    let s = scene("unreadable", &["a.pdf", "b.pdf"]);
    set_raw(&s.file("a.pdf"), b"opaque-not-a-property-list");
    let plan = plan_tags(&s, &[spec("pdf", "Work")]);
    assert_eq!(plan.tag_state_unreadable(), 1);
    assert!(
        plan.untouched
            .iter()
            .any(|(p, why)| p.ends_with("a.pdf") && *why == Untouched::TagStateUnreadable)
    );
    assert_eq!(plan.groups[0].members, vec![s.file("b.pdf")]);

    // A state that became unreadable after planning is skipped at apply too.
    let plan = plan_tags(&s, &[spec("pdf", "Work")]);
    set_raw(&s.file("b.pdf"), b"also-not-a-list");
    let report = apply_plan(&plan);
    assert_eq!(report.tagged, 0);
    assert_eq!(report.tag_skipped_unreadable, 1);
    assert_eq!(
        raw(&s.file("a.pdf")).as_deref(),
        Some(&b"opaque-not-a-property-list"[..])
    );
    assert_eq!(
        raw(&s.file("b.pdf")).as_deref(),
        Some(&b"also-not-a-list"[..])
    );
}

#[test]
fn tags_that_change_after_the_journal_is_written_are_left_as_found() {
    let _g = lock();
    let s = scene("race", &["a.pdf", "b.pdf"]);
    set_tags(&s.file("a.pdf"), &["Old"]);
    let plan = plan_tags(&s, &[spec("pdf", "Work")]);
    let a = s.file("a.pdf");
    let mut change = |index: usize, _: &Path| {
        if index == 0 {
            set_tags(&a, &["Old", "Someone Else"]);
        }
    };
    let report = apply::apply_with_faults_for_tests(
        &plan,
        "test",
        Some(&TestSeal),
        ApplyFaults {
            before_rename: Some(&mut change),
            after_rename: None,
            ..Default::default()
        },
    )
    .expect("apply");
    assert_eq!(report.tagged, 1);
    assert_eq!(report.tag_skipped_changed, 1);
    assert_eq!(
        tags_of(&s.file("a.pdf")).unwrap(),
        ["Old", "Someone Else"],
        "not overwritten"
    );
    let journal = reload(&report);
    assert_eq!(journal.entries[0].state, EntryState::Held);
}

#[test]
fn finder_comments_are_left_untouched_by_plan_apply_and_undo() {
    let _g = lock();
    let s = scene("comment", &["a.pdf"]);
    let comment = |path: &Path| {
        let out = Command::new("xattr")
            .args(["-px", COMMENT])
            .arg(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert!(
        Command::new("xattr")
            .args(["-w", COMMENT, "a private comment"])
            .arg(s.file("a.pdf"))
            .status()
            .unwrap()
            .success()
    );
    let before = comment(&s.file("a.pdf"));
    assert!(!before.is_empty());
    let report = apply_plan(&plan_tags(&s, &[spec("pdf", "Work")]));
    assert_eq!(comment(&s.file("a.pdf")), before);
    let mut journal = reload(&report);
    let _ = apply::undo(&mut journal, &TestSeal);
    assert_eq!(comment(&s.file("a.pdf")), before);
}

#[test]
fn the_safety_refusals_still_win_over_a_tag() {
    let _g = lock();
    let s = scene("refusal", &["Tax return 2025.pdf", "notes.pdf"]);
    let plan = plan_tags(&s, &[spec("pdf", "Work")]);
    assert_eq!(plan.groups[0].members, vec![s.file("notes.pdf")]);
    apply_plan(&plan);
    assert_eq!(raw(&s.file("Tax return 2025.pdf")), None, "never tagged");
    assert!(raw(&s.file("notes.pdf")).is_some());
}

#[test]
fn a_tag_plan_round_trips_through_an_exported_file_and_cannot_be_proposed() {
    let _g = lock();
    let s = scene("roundtrip", &["a.pdf", "b.pdf"]);
    set_tags(&s.file("a.pdf"), &["Old"]);
    let plan = plan_tags(&s, &[spec("pdf", "Work")]);
    let context = BindingContext::new("sweep", "0.5.4", "sweep-metadata-v1;scheme=tags", "c");
    let bound = BoundPlan::from_plan(plan, context.clone()).expect("binds");
    let file = s.base.join("plan.bin");
    bound.export(&file).expect("exports");
    let loaded = BoundPlan::load(&file, bound.digest()).expect("loads");
    assert!(loaded.plan.adds_tags);
    assert_eq!(loaded.plan.groups.len(), 1);
    assert_eq!(loaded.plan.groups[0].name, "Work");
    assert!(matches!(
        &loaded.plan.groups[0].signal,
        Signal::Tagged { ext, new: true, .. } if ext == "pdf"
    ));
    assert_eq!(loaded.digest(), bound.digest());
    loaded.validate(&context).expect("still valid");
    let refused = etude_core::proposal::propose(&loaded, b"{}").expect_err("refused");
    assert!(format!("{refused:?}").contains("tags plan"), "{refused:?}");
}

#[test]
fn a_changed_tag_set_makes_the_bound_plan_stale() {
    let _g = lock();
    let s = scene("stale", &["a.pdf"]);
    set_tags(&s.file("a.pdf"), &["Old"]);
    let plan = plan_tags(&s, &[spec("pdf", "Work")]);
    let context = BindingContext::new("sweep", "0.5.4", "sweep-metadata-v1;scheme=tags", "c");
    let bound = BoundPlan::from_plan(plan, context.clone()).expect("binds");
    bound.validate(&context).expect("fresh");
    std::thread::sleep(std::time::Duration::from_millis(20));
    set_tags(&s.file("a.pdf"), &["Old", "Changed"]);
    assert!(
        bound.validate(&context).is_err(),
        "a plan made before the tags changed is not applied"
    );
}

#[test]
fn adding_tags_without_a_journal_is_refused() {
    let _g = lock();
    let s = scene("nojournal", &["a.pdf"]);
    let plan = plan_tags(&s, &[spec("pdf", "Work")]);
    let error = apply::apply_with_structured_progress(&plan, "test", None, None, |_| {})
        .expect_err("refused");
    assert!(matches!(error, apply::ApplyError::Io(_)));
    assert_eq!(raw(&s.file("a.pdf")), None, "nothing was written");
}

#[test]
fn planned_tag_never_removes_an_identical_manual_tag() {
    let _g = lock();
    let s = scene("manual_planned", &["a.pdf"]);
    let failure = apply::apply_with_structured_progress_outcome(
        &plan_tags(&s, &[spec("pdf", "Work")]),
        "test",
        Some(&TestSeal),
        Some(0),
        |_| {},
    )
    .expect_err("stopped before writing tags");
    let mut journal =
        Journal::load_sealed("test", &failure.durable_journal_id.unwrap(), &TestSeal).unwrap();
    let written = journal.entries[0].tag.as_ref().unwrap().written.clone();
    set_raw(&s.file("a.pdf"), &written);
    let undone = apply::undo(&mut journal, &TestSeal);
    assert_eq!(raw(&s.file("a.pdf")), Some(written));
    assert_eq!(undone.skipped_changed, vec![s.file("a.pdf")]);
    assert!(!apply::has_undo_work(&journal, &TestSeal));
}

#[test]
fn undo_preserves_a_replacement_files_identical_tags() {
    let _g = lock();
    let s = scene("replaced_identity", &["a.pdf"]);
    let report = apply_plan(&plan_tags(&s, &[spec("pdf", "Work")]));
    let mut journal = reload(&report);
    let written = raw(&s.file("a.pdf")).unwrap();
    fs::rename(s.file("a.pdf"), s.file("original")).unwrap();
    fs::write(s.file("a.pdf"), b"replacement file").unwrap();
    set_raw(&s.file("a.pdf"), &written);
    let undone = apply::undo(&mut journal, &TestSeal);
    assert_eq!(raw(&s.file("a.pdf")), Some(written));
    assert_eq!(undone.skipped_changed, vec![s.file("a.pdf")]);
}

#[test]
fn older_tag_journals_with_zero_identity_restore_unchanged_tags() {
    let _g = lock();
    let s = scene("legacy_zero_identity", &["a.pdf"]);
    let report = apply_plan(&plan_tags(&s, &[spec("pdf", "Work")]));
    let mut journal = reload(&report);
    for entry in &mut journal.entries {
        entry.size = 0;
        entry.mtime_secs = 0;
        entry.inode = 0;
        entry.edge_hash = 0;
    }
    let legacy = "sweep-journal 1\n".to_string()
        + &journal
            .encode()
            .lines()
            .skip(2)
            .collect::<Vec<_>>()
            .join("\n")
        + "\n";
    let mut legacy = Journal::decode(&legacy).unwrap();
    legacy.id = journal.id;
    legacy.tool = journal.tool;
    let undone = apply::undo(&mut legacy, &TestSeal);
    assert!(undone.error.is_none());
    assert_eq!(undone.tags_restored, 1);
    assert_eq!(raw(&s.file("a.pdf")), None);
}
