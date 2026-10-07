// Shared integration-test body, included by each CLI. Probe CARGO_BIN_EXE,
// rather than a possibly stale binary found through PATH.

#[test]
fn capability_contract_matches_the_shipped_binary() {
    run_contract_witness(false);
}

#[test]
fn contract_witness_rejects_unversioned_schema_and_observed_scope_drift() {
    run_contract_witness(true);
}

fn run_contract_witness(negative_controls: bool) {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/check-tool-contracts.py");
    let mut command = std::process::Command::new("python3");
    command.arg(script).args(["--tool", CONTRACT_TOOL, "--bin", CONTRACT_BINARY]);
    if negative_controls {
        command.arg("--negative-controls");
    }
    let output = command
        .output()
        .expect("python3 is required for the independent contract witness");
    assert!(
        output.status.success(),
        "contract witness failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
