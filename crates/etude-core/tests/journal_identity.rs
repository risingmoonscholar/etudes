use etude_core::journal::{self, Entry, EntryState, Journal, JournalError, Method, Sealer};
use std::cell::Cell;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

static ENVIRONMENT: Mutex<()> = Mutex::new(());
static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn lock() -> MutexGuard<'static, ()> {
    ENVIRONMENT
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "etude-journal-identity-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        unsafe {
            std::env::set_var("ETUDE_STATE_DIR", &root);
        }
        Self(root)
    }
    fn journal(&self, id: &str) -> Journal {
        Journal {
            id: id.into(),
            tool: "stash".into(),
            root: self.0.join("synthetic-selected-tree"),
            entries: vec![Entry {
                from: self.0.join("synthetic-source"),
                to: self.0.join("synthetic-destination"),
                method: Method::Rename,
                size: 0,
                mtime_secs: 0,
                inode: 0,
                edge_hash: 0,
                state: EntryState::Planned,
            }],
            progress_tail_damaged: false,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
        unsafe {
            std::env::remove_var("ETUDE_STATE_DIR");
        }
    }
}

#[derive(Default)]
struct TestSeal {
    opens: Cell<usize>,
}
impl TestSeal {
    fn tag(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0x746573742d6b6579u64, |tag, byte| {
            tag.rotate_left(5).wrapping_add(u64::from(*byte))
        })
    }
}
impl Sealer for TestSeal {
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, &'static str> {
        let mut out = Self::tag(plain).to_le_bytes().to_vec();
        out.extend(plain.iter().map(|byte| byte ^ 0x5a));
        Ok(out)
    }
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
        self.opens.set(self.opens.get() + 1);
        let tag = sealed.get(..8).ok_or("synthetic short seal")?;
        let plain: Vec<_> = sealed[8..].iter().map(|byte| byte ^ 0x5a).collect();
        if tag != Self::tag(&plain).to_le_bytes() {
            return Err("synthetic authentication failure");
        }
        Ok(plain)
    }
}

#[test]
fn sealed_stash_identity_survives_decode_and_resave() {
    let _guard = lock();
    let fixture = Fixture::new();
    let seal = TestSeal::default();
    let journal = fixture.journal("batch-a");
    let decoded = Journal::decode(&journal.encode()).unwrap();
    assert_eq!(decoded.id, journal.id);
    assert_eq!(decoded.tool, journal.tool);
    journal.save_sealed(&seal).unwrap();
    let loaded = Journal::load_sealed_exact("stash", "batch-a", &seal).unwrap();
    loaded.save_sealed(&seal).unwrap();
    assert_eq!(
        Journal::load_sealed_exact("stash", "batch-a", &seal)
            .unwrap()
            .id,
        "batch-a"
    );
    assert!(
        !fs::read(journal.path())
            .unwrap()
            .windows(7)
            .any(|window| window == b"batch-a")
    );
}

#[test]
fn copied_base_is_rejected_before_any_progress_replay() {
    let _guard = lock();
    let fixture = Fixture::new();
    let seal = TestSeal::default();
    let a = fixture.journal("batch-a");
    let b = fixture.journal("batch-b");
    a.save_sealed(&seal).unwrap();
    a.record_done(0, Method::Rename, &seal).unwrap();
    b.save_sealed(&seal).unwrap();
    fs::copy(a.path(), b.path()).unwrap();
    seal.opens.set(0);
    let error = Journal::load_sealed_exact("stash", "batch-b", &seal).unwrap_err();
    assert!(matches!(
        error,
        JournalError::Malformed("journal identity does not match selector")
    ));
    assert_eq!(seal.opens.get(), 1);
}

#[test]
fn copied_stash_under_another_tool_is_refused() {
    let _guard = lock();
    let fixture = Fixture::new();
    let seal = TestSeal::default();
    let journal = fixture.journal("batch-a");
    journal.save_sealed(&seal).unwrap();
    fs::copy(journal.path(), fixture.0.join("sweep-batch-a.journal")).unwrap();
    assert!(matches!(
        Journal::load_sealed_exact("sweep", "batch-a", &seal),
        Err(JournalError::Malformed(
            "journal identity does not match selector"
        ))
    ));
}

#[test]
fn unsafe_exact_ids_refuse_before_filesystem_observations() {
    let _guard = lock();
    let _fixture = Fixture::new();
    let seal = TestSeal::default();
    etude_core::scan::reset_receipt();
    for id in [
        "",
        ".",
        "..",
        "../batch-a",
        "/absolute",
        "a/b",
        "a\\b",
        "a\0b",
        "a\nb",
        "-a",
    ] {
        assert!(!journal::valid_journal_id(id));
        assert!(Journal::load_sealed_exact("stash", id, &seal).is_err());
    }
    assert!(!journal::valid_journal_id(&"a".repeat(129)));
    assert!(Journal::load_sealed_exact("../stash", "batch-a", &seal).is_err());
    assert_eq!(seal.opens.get(), 0);
    assert!(etude_core::scan::receipt_json().contains("\"categories\":[]"));
}

#[test]
fn legacy_stash_loads_and_upgrades_identity_when_resaved() {
    let _guard = lock();
    let fixture = Fixture::new();
    let seal = TestSeal::default();
    let journal = fixture.journal("legacy-a");
    let legacy = journal.encode().replace(
        "sweep-journal 2\nidentity\tstash\tlegacy-a\n",
        "sweep-journal 1\n",
    );
    let sealed = seal.seal(legacy.as_bytes()).unwrap();
    fs::write(journal.path(), sealed).unwrap();
    let loaded = Journal::load_sealed_exact("stash", "legacy-a", &seal).unwrap();
    assert_eq!(loaded.id, "legacy-a");
    assert_eq!(loaded.root, journal.root);
    loaded.save_sealed(&seal).unwrap();
    assert!(
        loaded
            .encode()
            .starts_with("sweep-journal 2\nidentity\tstash\tlegacy-a\n")
    );
    Journal::load_sealed_exact("stash", "legacy-a", &seal).unwrap();
}

#[test]
fn exact_loading_ignores_unrelated_ties_and_damaged_journals() {
    let _guard = lock();
    let fixture = Fixture::new();
    let seal = TestSeal::default();
    let a = fixture.journal("batch-a");
    let b = fixture.journal("batch-b");
    a.save_sealed(&seal).unwrap();
    b.save_sealed(&seal).unwrap();
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(123);
    for path in [a.path(), b.path()] {
        fs::File::open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(time))
            .unwrap();
    }
    assert!(journal::candidates_by_recency("stash").is_err());
    assert_eq!(journal::candidates_unordered("stash").unwrap().len(), 2);
    fs::write(
        fixture.0.join("stash-damaged.journal"),
        b"synthetic damaged journal",
    )
    .unwrap();
    assert_eq!(journal::candidates_unordered("stash").unwrap().len(), 3);
    assert_eq!(
        Journal::load_sealed_exact("stash", "batch-a", &seal)
            .unwrap()
            .id,
        "batch-a"
    );
}

#[cfg(unix)]
#[test]
fn unordered_inventory_keeps_the_symlink_barrier() {
    let _guard = lock();
    let fixture = Fixture::new();
    let seal = TestSeal::default();
    let a = fixture.journal("batch-a");
    a.save_sealed(&seal).unwrap();
    std::os::unix::fs::symlink(a.path(), fixture.0.join("stash-link.journal")).unwrap();
    assert!(journal::candidates_unordered("stash").is_err());
    assert!(Journal::load_sealed_exact("stash", "link", &seal).is_err());
    Journal::load_sealed_exact("stash", "batch-a", &seal).unwrap();
}
