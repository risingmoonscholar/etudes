//! Manual crash-safety proof, not part of the automated suite. Applies 500
//! files with the journal on, then the driving shell script SIGKILLs this
//! process partway through. A companion `sigkill_undo` example then loads
//! the journal and runs undo, proving recorded moves are still reversible
//! after a real process kill (not just an in-process injected error).
use std::fs;

use etude_core::apply;
use etude_core::journal::Sealer;
use etude_core::plan::{self, Plan};

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
    let root = std::path::PathBuf::from(std::env::var("SIGKILL_ROOT").expect("SIGKILL_ROOT"));
    let n: usize = std::env::var("SIGKILL_N")
        .unwrap_or_else(|_| "500".into())
        .parse()
        .unwrap();
    fs::create_dir_all(&root).expect("mkdir root");
    let members: Vec<_> = (0..n)
        .map(|i| {
            let p = root.join(format!("file_{i:05}.bin"));
            fs::write(&p, format!("payload {i}\n")).expect("write member");
            p
        })
        .collect();
    let plan = Plan {
        root: root.clone(),
        groups: vec![plan::Group {
            name: "Bench".to_string(),
            signal: plan::Signal::Screenshot,
            members,
            accepted: true,
        }],
        untouched: Vec::new(),
        scanned: n,
        skipped_hidden: 0,
        skipped_symlink: 0,
        skipped_system: 0,
        skipped_project: 0,
        skipped_unreadable: 0,
        root_is_synced: false,
        allow_sync: false,
    };
    println!(
        "sigkill_apply: starting apply of {n} files, pid={}",
        std::process::id()
    );
    let rep = apply::apply(&plan, "sigkilltest", Some(&TestSeal), None).expect("apply");
    println!(
        "sigkill_apply: completed without being killed, moved={}",
        rep.moved
    );
}
