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
use etude_core::plan::{self, BindingContext, BoundPlan, Plan};
use etude_core::scan::{self, ScanConfig};

fn context() -> BindingContext {
    BindingContext::new(
        "bench",
        env!("CARGO_PKG_VERSION"),
        "fixture-v1",
        "metadata-v1",
    )
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

const N: usize = 2000;

/// Build and bind a synthetic proposal before timing apply and journal cost.
fn build_bench_plan(root: &std::path::Path, n: usize) -> BoundPlan {
    fs::create_dir_all(root).expect("mkdir root");
    let members: Vec<_> = (0..n)
        .map(|i| {
            let p = root.join(format!("file_{i:05}.bin"));
            fs::write(&p, format!("payload {i}\n")).expect("write member");
            p
        })
        .collect();
    {
        let out = scan::scan(root, &ScanConfig::default()).expect("scan benchmark fixture");
        let proposal = Plan::with_groups(
            &out,
            vec![plan::Group {
                name: "Bench".to_string(),
                signal: plan::Signal::Screenshot,
                members,
                accepted: true,
            }],
        );
        BoundPlan::from_plan(proposal, context()).expect("bind benchmark fixture")
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
    let rep_off = apply::apply(&plan_off, &context(), None, None).expect("apply no-journal");
    let dt_off = t0.elapsed();
    assert_eq!(rep_off.moved, N);

    let root_on = base.join("with_journal");
    let plan_on = build_bench_plan(&root_on, N);
    let state_on = base.join("state_on");
    unsafe { std::env::set_var("ETUDE_STATE_DIR", &state_on) };
    let t1 = Instant::now();
    let rep_on = apply::apply(&plan_on, &context(), Some(&TestSeal), None).expect("apply journal");
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
