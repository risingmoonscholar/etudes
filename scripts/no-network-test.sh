#!/bin/sh
# M8: run the whole test suite with networking structurally impossible.
#
# This is the strongest form of sweep's central claim. `deny.toml` stops a
# networking crate entering the tree and the symbol scan checks the shipped
# binary, but both inspect the code. This runs it: if any part of sweep tried to
# open a socket during the suite, the call fails and the test fails with it.
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
echo "==> running the suite with networking denied"
sandbox-exec -f "$PROFILE" cargo test --offline --quiet
status=$?

if [ $status -eq 0 ]; then
    echo
    echo "PASS: the full suite completed with socket access denied by the OS."
    echo "      A network call anywhere in sweep would have failed a test."
else
    echo "FAIL: the suite did not pass under network denial." >&2
fi
exit $status
