const CONTRACT_TOOL: &str = "unpack";
const CONTRACT_BINARY: &str = env!("CARGO_BIN_EXE_unpack");
#[path = "../../etude-cli-support/test-support/test_tool.rs"]
mod test_tool;
include!("../../etude-cli-support/test-support/contract_probe.rs");
