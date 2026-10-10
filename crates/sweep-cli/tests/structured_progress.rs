use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "sweep-progress-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(path.join("root")).unwrap();
        Self(path)
    }
    fn run(&self, count: usize, journal: bool) {
        for n in 0..count {
            std::fs::write(
                self.0.join("root").join(format!("Screenshot {n:04}.png")),
                b"synthetic",
            )
            .unwrap();
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_sweep"));
        command
            .args([
                "apply",
                self.0.join("root").to_str().unwrap(),
                "--yes",
                "--since",
                "0",
                "--json",
                "--progress-json",
            ])
            .env("ETUDE_STATE_DIR", self.0.join("state"))
            .env("ETUDE_JOURNAL_KEY", "11".repeat(32))
            .env("SWEEP_GRACE_SECS", "0");
        if !journal {
            command.arg("--no-journal");
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(if count == 0 { 1 } else { 0 }),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut witness=Command::new("python3").args(["-c",r#"
import json,sys
payload=json.load(sys.stdin)
result=json.loads(payload['stdout'])
records=[json.loads(line) for line in payload['stderr'].splitlines() if line.startswith('{')]
assert 1<=len(records)<=12,records
keys={'schema_version','event','tool_version','operation_id','operation','phase','planned','completed','journalled'}
last_completed=last_journalled=0
for row in records:
 assert set(row)==keys,row
 assert row['schema_version']==1 and row['event']=='progress' and row['operation']=='apply'
 assert row['operation_id']==result['operation_id'] and row['tool_version']==result['tool_version']
 assert all(type(row[k]) is int for k in ('planned','completed','journalled'))
 assert last_completed<=row['completed']<=row['planned']
 assert last_journalled<=row['journalled']<=row['completed']
 last_completed,last_journalled=row['completed'],row['journalled']
last=records[-1]
assert last['phase']=='done' and last['planned']==payload['count'] and last['completed']==payload['count'],last
assert last['journalled']==(payload['count'] if payload['journal'] else 0),last
"#]).stdin(Stdio::piped()).spawn().unwrap();
        let payload = etude_core::json::obj(&[
            (
                "stdout",
                etude_core::json::str(&String::from_utf8(output.stdout).unwrap()),
            ),
            (
                "stderr",
                etude_core::json::str(&String::from_utf8(output.stderr).unwrap()),
            ),
            ("count", etude_core::json::num(count)),
            ("journal", etude_core::json::bool(journal)),
        ]);
        witness
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        assert!(witness.wait().unwrap().success());
        assert_eq!(
            std::fs::read_dir(self.0.join("root"))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_file())
                .count(),
            0
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn structured_events_pin_schema_counts_correlation_and_bound() {
    Fixture::new().run(250, true);
}
#[test]
fn no_journal_counts_are_zero_without_hiding_successful_moves() {
    Fixture::new().run(5, false);
}
#[test]
fn empty_apply_has_a_terminal_zero_event() {
    Fixture::new().run(0, true);
}
