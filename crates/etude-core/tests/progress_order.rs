use etude_core::apply::{self, ApplyError, ProgressPhase, StructuredProgress};
use etude_core::journal::{self, EntryState, Journal, Sealer};
use etude_core::plan::{BindingContext, BoundPlan, Group, Plan, Signal};
use etude_core::scan::{self, ScanConfig};
use std::cell::Cell;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

static ENVIRONMENT: Mutex<()> = Mutex::new(());
static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
const TOOL: &str = "progress-test";

fn lock() -> MutexGuard<'static, ()> {
    ENVIRONMENT
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

struct Fixture {
    base: PathBuf,
    plan: BoundPlan,
    context: BindingContext,
}
impl Fixture {
    fn new(count: usize) -> Self {
        let base = std::env::temp_dir().join(format!(
            "etude-progress-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("tree")).unwrap();
        let root = base.join("tree").canonicalize().unwrap();
        let members = (0..count)
            .map(|number| {
                let path = root.join(format!("file{number}.txt"));
                fs::write(&path, format!("synthetic fixture {number}")).unwrap();
                path
            })
            .collect();
        unsafe {
            std::env::set_var("ETUDE_STATE_DIR", base.join("state"));
        }
        let outcome = scan::scan(
            &root,
            &ScanConfig {
                depth: 1,
                grace: None,
                ..Default::default()
            },
        )
        .unwrap();
        let plan = Plan::with_groups(
            &outcome,
            vec![Group {
                name: "Destination".into(),
                signal: Signal::Collected { count },
                members,
                accepted: true,
            }],
        );
        let context =
            BindingContext::new(TOOL, "0.0.0", "progress-test-v1", "progress-test-contract");
        let plan = BoundPlan::from_plan(plan, context.clone()).unwrap();
        Self {
            base,
            plan,
            context,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
        unsafe {
            std::env::remove_var("ETUDE_STATE_DIR");
        }
    }
}

struct TestSeal {
    calls: Cell<usize>,
    fail_call: Option<usize>,
}
impl TestSeal {
    fn new(fail_call: Option<usize>) -> Self {
        Self {
            calls: Cell::new(0),
            fail_call,
        }
    }
}
impl Sealer for TestSeal {
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, &'static str> {
        let call = self.calls.get() + 1;
        self.calls.set(call);
        if self.fail_call == Some(call) {
            return Err("synthetic seal failure");
        }
        Ok(plain.iter().map(|byte| byte ^ 0x5a).collect())
    }
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, &'static str> {
        Ok(sealed.iter().map(|byte| byte ^ 0x5a).collect())
    }
}

fn terminal(
    events: &[StructuredProgress],
    phase: ProgressPhase,
    planned: usize,
    completed: usize,
    journalled: usize,
) {
    assert_eq!(
        events.last(),
        Some(&StructuredProgress {
            planned,
            completed,
            journalled,
            phase
        })
    );
}

#[test]
fn advancing_arrives_after_durable_record_and_before_next_move() {
    let _guard = lock();
    let fixture = Fixture::new(4);
    let seal = TestSeal::new(None);
    let mut events = Vec::new();
    let report = apply::apply_with_structured_progress(
        &fixture.plan.plan,
        TOOL,
        Some(&seal),
        None,
        |event| {
            if event.phase == ProgressPhase::Advancing {
                let id = journal::latest_id(TOOL).unwrap();
                let journal = Journal::load_sealed(TOOL, &id, &seal).unwrap();
                assert_eq!(
                    journal
                        .entries
                        .iter()
                        .filter(|entry| entry.state == EntryState::Moved)
                        .count(),
                    event.journalled
                );
                assert_eq!(event.completed, event.journalled);
                for entry in &journal.entries[..event.completed] {
                    assert!(!entry.from.exists());
                    assert!(entry.to.exists());
                }
                for entry in &journal.entries[event.completed..] {
                    assert!(entry.from.exists());
                    assert!(!entry.to.exists());
                }
            }
            events.push(event);
        },
    )
    .unwrap();
    assert_eq!(report.moved, 4);
    assert_eq!(
        events.first(),
        Some(&StructuredProgress {
            planned: 4,
            completed: 0,
            journalled: 0,
            phase: ProgressPhase::Planning
        })
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.phase == ProgressPhase::Advancing)
            .count(),
        4
    );
    terminal(&events, ProgressPhase::Done, 4, 4, 4);
}

#[test]
fn failed_done_record_reports_actual_move_without_claiming_journal_acknowledgement() {
    let _guard = lock();
    let fixture = Fixture::new(4);
    let seal = TestSeal::new(Some(2));
    let mut events = Vec::new();
    let error = apply::apply_with_structured_progress(
        &fixture.plan.plan,
        TOOL,
        Some(&seal),
        None,
        |event| events.push(event),
    )
    .unwrap_err();
    assert!(matches!(error, ApplyError::Journal(_)));
    terminal(&events, ProgressPhase::Error, 4, 1, 0);
    assert!(
        events
            .iter()
            .all(|event| event.phase != ProgressPhase::Advancing)
    );
    assert!(!fixture.plan.groups[0].members[0].exists());
    assert!(fixture.plan.groups[0].members[1].exists());
    let id = journal::latest_id(TOOL).unwrap();
    let journal = Journal::load_sealed(TOOL, &id, &seal).unwrap();
    assert!(
        journal
            .entries
            .iter()
            .all(|entry| entry.state == EntryState::Planned)
    );
}

#[test]
fn no_journal_moves_report_zero_durable_records() {
    let _guard = lock();
    let fixture = Fixture::new(4);
    let mut events = Vec::new();
    apply::apply_with_structured_progress(&fixture.plan.plan, TOOL, None, None, |event| {
        events.push(event)
    })
    .unwrap();
    assert!(events.iter().all(|event| event.journalled == 0));
    terminal(&events, ProgressPhase::Done, 4, 4, 0);
}

#[test]
fn injected_failure_reports_prior_durable_prefix() {
    let _guard = lock();
    let fixture = Fixture::new(4);
    let seal = TestSeal::new(None);
    let mut events = Vec::new();
    let error = apply::apply_with_structured_progress(
        &fixture.plan.plan,
        TOOL,
        Some(&seal),
        Some(2),
        |event| events.push(event),
    )
    .unwrap_err();
    assert!(matches!(error, ApplyError::Injected(2)));
    terminal(&events, ProgressPhase::Error, 4, 2, 2);
    assert!(fixture.plan.groups[0].members[2].exists());
}

#[test]
fn empty_operation_still_reports_terminal_done() {
    let _guard = lock();
    let fixture = Fixture::new(0);
    let mut events = Vec::new();
    apply::apply_with_structured_progress(&fixture.plan.plan, TOOL, None, None, |event| {
        events.push(event)
    })
    .unwrap();
    terminal(&events, ProgressPhase::Done, 0, 0, 0);
}

#[test]
fn base_journal_failure_reports_validated_plan_with_zero_moves() {
    let _guard = lock();
    let fixture = Fixture::new(4);
    let seal = TestSeal::new(Some(1));
    let mut events = Vec::new();
    let error = apply::apply_with_structured_progress(
        &fixture.plan.plan,
        TOOL,
        Some(&seal),
        None,
        |event| events.push(event),
    )
    .unwrap_err();
    assert!(matches!(error, ApplyError::Journal(_)));
    terminal(&events, ProgressPhase::Error, 4, 0, 0);
    assert!(
        fixture.plan.groups[0]
            .members
            .iter()
            .all(|path| path.exists())
    );
}

#[test]
fn legacy_callback_keeps_one_event_per_acknowledged_move() {
    let _guard = lock();
    let fixture = Fixture::new(4);
    let seal = TestSeal::new(Some(3));
    let mut completed = Vec::new();
    assert!(
        apply::apply_with_progress(
            &fixture.plan,
            &fixture.context,
            Some(&seal),
            None,
            |event| completed.push((event.completed, event.total))
        )
        .is_err()
    );
    assert_eq!(completed, vec![(1, 4)]);
    assert!(fixture.plan.groups[0].members[2].exists());
}

#[test]
fn preflight_refusal_reports_terminal_error_without_claiming_moves() {
    let _guard = lock();
    let fixture = Fixture::new(4);
    let destination = fixture.plan.root.join("Destination");
    fs::create_dir(&destination).unwrap();
    fs::write(
        destination.join("file0.txt"),
        b"synthetic existing destination",
    )
    .unwrap();
    let mut events = Vec::new();
    let error =
        apply::apply_with_structured_progress(&fixture.plan.plan, TOOL, None, None, |event| {
            events.push(event)
        })
        .unwrap_err();
    assert!(matches!(error, ApplyError::DestinationExists(_)));
    assert_eq!(events.len(), 1);
    terminal(&events, ProgressPhase::Error, 0, 0, 0);
    assert!(
        fixture.plan.groups[0]
            .members
            .iter()
            .all(|path| path.exists())
    );
}
