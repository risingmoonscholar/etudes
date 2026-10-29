// Pack runs the macOS tar and xattr, so the archive probes are macOS only.
#![cfg(target_os = "macos")]

const CONTRACT_TOOL: &str = "pack";
const CONTRACT_BINARY: &str = env!("CARGO_BIN_EXE_pack");
include!("../../etude-cli-support/test-support/contract_probe.rs");
