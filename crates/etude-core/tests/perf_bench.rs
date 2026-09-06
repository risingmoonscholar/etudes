//! Manual benchmark for the apply/journal hot path. Not run by default
//! (`#[ignore]`). It moves 2000 real files on disk and times it. That is
//! slow and not something CI should pay for on every push. Run explicitly:
//!
//!   cargo test -p etude-core --test perf_bench -- --ignored --nocapture
//!
//! This exists to put a real number on the "journal makes apply ~14x
//! slower" claim, and to re-check that number after a fix, on real hardware
//! rather than a guess.

use std::fs;
use std::time::Instant;

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

const N: usize = 2000;

/// Build `n` small real files directly under `root` and a manual Plan that
/// moves every one of them into a single destination group. Bypasses
/// scan/classify on purpose: this measures apply()/journal cost, not
/// classification cost.
fn build_bench_plan(root: &std::path::Path, n: usize) -> Plan {
    fs::create_dir_all(root).expect("mkdir root");
    let members: Vec<_> = (0..n)
        .map(|i| {
            let p = root.join(format!("file_{i:05}.bin"));
            fs::write(&p, format!("payload {i}\n")).expect("write member");
            p
        })
        .collect();
    Plan {
        root: root.to_path_buf(),
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
        skipped_in_flight: 0,
        skipped_package: 0,
        skipped_unreadable: 0,
        skipped_tagged: 0,
        include_tagged: false,
        root_is_synced: false,
        allow_sync: false,
    }
}

#[test]
// Builds 4,000 files across two trees, so it is too slow for every `cargo test`
// run. It is a measurement rather than an assertion about correctness: run it
// with `cargo test -- --ignored` when changing anything on the journal path.
#[ignore = "benchmark: builds 4,000 files; run with --ignored"]
fn apply_journal_overhead_2000_files() {
    let base = std::env::temp_dir().join(format!("sweep_bench_{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);

    let root_off = base.join("no_journal");
    let plan_off = build_bench_plan(&root_off, N);
    let state_off = base.join("state_off");
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state_off) };
    let t0 = Instant::now();
    let rep_off = apply::apply(&plan_off, "bench", None, None).expect("apply no-journal");
    let dt_off = t0.elapsed();
    assert_eq!(rep_off.moved, N);

    let root_on = base.join("with_journal");
    let plan_on = build_bench_plan(&root_on, N);
    let state_on = base.join("state_on");
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state_on) };
    let t1 = Instant::now();
    let rep_on = apply::apply(&plan_on, "bench", Some(&TestSeal), None).expect("apply journal");
    let dt_on = t1.elapsed();
    assert_eq!(rep_on.moved, N);

    let ratio = dt_on.as_secs_f64() / dt_off.as_secs_f64().max(1e-9);
    println!(
        "N={N}  no-journal={:.3}s  journal={:.3}s  ratio={:.1}x",
        dt_off.as_secs_f64(),
        dt_on.as_secs_f64(),
        ratio
    );

    let _ = fs::remove_dir_all(&base);
    unsafe { std::env::remove_var("ETUDE_STATE_DIR") };
}

#[test]
#[ignore = "benchmark: creates 2000 files and compares metadata probe cost"]
fn scan_finder_tag_probe_overhead_2000_files() {
    use etude_core::scan::{ScanConfig, scan};
    let root = std::env::temp_dir().join(format!("sweep_scan_bench_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    build_bench_plan(&root, N);
    let mut elapsed = [std::time::Duration::ZERO; 2];
    // Alternate order to reduce systematic warm-cache bias.
    for round in 0..10 {
        for slot in [round % 2, 1 - round % 2] {
            let start = Instant::now();
            let outcome = scan(
                &root,
                &ScanConfig {
                    grace: None,
                    include_tagged: slot == 1,
                    ..ScanConfig::default()
                },
            )
            .unwrap();
            elapsed[slot] += start.elapsed();
            assert_eq!(outcome.entries.len(), N);
            assert_eq!(outcome.skipped_unreadable, 0);
            assert_eq!(outcome.skipped_tagged, 0);
        }
    }
    println!(
        "N={N} mean scan: tag-probe={:.3}ms opt-in-no-probe={:.3}ms ratio={:.2}x",
        elapsed[0].as_secs_f64() * 100.0,
        elapsed[1].as_secs_f64() * 100.0,
        elapsed[0].as_secs_f64() / elapsed[1].as_secs_f64()
    );
    fs::remove_dir_all(root).unwrap();
}
