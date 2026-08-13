//! Companion to sigkill_apply: loads the most recent journal written by
//! "sigkilltest" and runs undo, printing what it restored. Manual
//! crash-safety proof, not part of the automated suite.
use etude_core::apply;
use etude_core::journal::{Journal, Sealer};

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

fn main() {
    let mut j = Journal::latest_sealed("sigkilltest", &TestSeal).expect("load journal");
    let done_before = j.entries.iter().filter(|e| e.is_moved()).count();
    println!(
        "sigkill_undo: journal has {} entries, {} marked done",
        j.entries.len(),
        done_before
    );
    let r = apply::undo(&mut j);
    println!(
        "sigkill_undo: restored={} skipped_changed={} skipped_missing={} error={:?}",
        r.restored,
        r.skipped_changed.len(),
        r.skipped_missing.len(),
        r.error
    );
}
