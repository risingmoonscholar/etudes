#!/bin/sh
# M8: run the test suite with networking structurally impossible. On Darwin,
# the process-control witness runs outside denial because Seatbelt hides children.
#
# `deny.toml` stops a networking crate entering the tree and the symbol scan
# checks the shipped binary, but both inspect the code. This runs the tests with
# sockets denied; the Darwin process-control test makes no network calls and is
# run separately because Seatbelt hides its child process from the observer.
#
# The script PROVES THE SANDBOX WORKS before trusting it. A sandbox that
# silently stopped denying network would turn this into a green light that
# checks nothing, which is worse than having no check at all.
#
# Usage:  scripts/no-network-test.sh
# Exit:   0 all green under denial · 1 suite failed · 2 sandbox not trustworthy

set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
PROFILE="$ROOT/scripts/no-network.sb"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

if [ ! -f "$PROFILE" ]; then
    echo "no-network: missing $PROFILE" >&2
    exit 2
fi

command -v sandbox-exec >/dev/null 2>&1 || {
    echo "no-network: sandbox-exec unavailable; cannot make this claim on this host" >&2
    exit 2
}

# ---------------------------------------------------------------- step 1 of 3
# Build a control program that deliberately opens a socket.
echo "==> proving the sandbox actually denies network"
cat > "$WORK/probe.rs" <<'PROBE'
use std::net::TcpStream;
fn main() {
    match TcpStream::connect("1.1.1.1:53") {
        Ok(_) => std::process::exit(1),  // reached the network
        Err(_) => std::process::exit(0), // denied, as required
    }
}
PROBE
rustc -O -o "$WORK/probe" "$WORK/probe.rs" 2>/dev/null || {
    echo "no-network: could not build the control probe" >&2
    exit 2
}

# The probe must SUCCEED at connecting when unsandboxed. If it cannot reach the
# network anyway, the sandbox proves nothing and this host cannot witness it.
if "$WORK/probe"; then
    echo "no-network: host has no network access, so denial proves nothing" >&2
    echo "            run this on a machine that is online" >&2
    exit 2
fi

# And it must FAIL when sandboxed. This is the load-bearing check.
if ! sandbox-exec -f "$PROFILE" "$WORK/probe"; then
    echo "no-network: sandbox did NOT block a socket. the profile is broken" >&2
    exit 2
fi
echo "    ok: control probe connects unsandboxed, is denied under the profile"

# ---------------------------------------------------------------- step 2 of 3
# Build first, outside the sandbox. Compilation may legitimately touch the
# registry; the claim is about sweep at RUN time, not about cargo.
echo "==> building (outside the sandbox; the claim is about runtime)"
cargo build --tests --quiet

# ---------------------------------------------------------------- step 3 of 3
if [ "$(uname -s)" = Darwin ]; then
  echo "==> running the suite with networking denied (excluding process-control witness)"
  # Seatbelt hides child-process enumeration from this one process-control
  # test, which makes no network calls. Keep every other test under denial,
  # then run the focused witness outside the sandbox with that exception clear.
  sandbox-exec -f "$PROFILE" cargo test --offline --quiet -- \
    --skip sigterm_stops_extractor_cleans_staging_and_reports_final_result || {
      status=$?
      echo "FAIL: the denied suite failed before the process-control test." >&2
      exit "$status"
    }
  echo "==> running the process-control test outside network denial (no network calls)"
  cargo test --offline --quiet -p unpack-cli --test sigterm -- \
    sigterm_stops_extractor_cleans_staging_and_reports_final_result || {
      status=$?
      echo "FAIL: the process-control test failed outside network denial." >&2
      exit "$status"
    }
else
  echo "==> running the full suite with networking denied"
  sandbox-exec -f "$PROFILE" cargo test --offline --quiet
fi
status=$?

if [ $status -eq 0 ]; then
    echo
  if [ "$(uname -s)" = Darwin ]; then
    echo "PASS: all other tests completed with socket access denied by the OS."
    echo "      The process-control test ran outside denial; it makes no network calls."
  else
    echo "PASS: the full suite completed with socket access denied by the OS."
    echo "      A network call anywhere in sweep would have failed a test."
  fi
else
    echo "FAIL: the suite did not pass under network denial." >&2
fi
exit $status
