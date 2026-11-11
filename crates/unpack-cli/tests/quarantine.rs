// The quarantine matrix is macOS only, and so is its use of the helper.
#[cfg(target_os = "macos")]
#[path = "../../etude-cli-support/test-support/test_tool.rs"]
mod test_tool;
#[cfg(target_os = "macos")]
use test_tool::with_test_key;

#[cfg(target_os = "macos")]
fn matrix(mode: &str) {
    let output = with_test_key(&mut std::process::Command::new("python3"))
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/quarantine_matrix.py"))
        .arg(mode)
        .arg(env!("CARGO_BIN_EXE_unpack"))
        .output()
        .expect("run quarantine matrix");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
#[cfg(target_os = "macos")]
fn every_system_extractor_and_alias_has_measured_quarantine_propagation() {
    matrix("raw");
}
#[test]
#[cfg(target_os = "macos")]
fn unpack_preserves_quarantine_through_anchoring_and_flattening_for_every_format() {
    matrix("unpack");
}
