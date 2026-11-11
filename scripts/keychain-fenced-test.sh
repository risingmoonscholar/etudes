#!/bin/sh
# Run the test suite where the keychain program cannot start, so no test can
# read or create a key in the login keychain of whoever runs it.
#
# Every test gives the tools a key of its own (see
# crates/etude-cli-support/test-support/test_tool.rs, and
# scripts/check-test-keys.py, which fails a test that does not). This is the
# second half of that promise: if a test still reached for the keychain, here
# it finds none instead of the owner's.
#
# The script PROVES THE FENCE WORKS before trusting it, on a harmless program.
# The keychain program itself is never started, not even as a control.
#
# Usage:  scripts/keychain-fenced-test.sh [extra cargo test arguments]
# Exit:   0 all green inside the fence · 1 suite failed · 2 fence not trustworthy

set -eu

# Keep this equal to KEYCHAIN_FENCE in test_tool.rs. A sandbox can be applied
# again inside itself only when the profile is the same, and the tests that run
# a tool with no key apply this profile themselves.
FENCE='(version 1)(allow default)(deny process-exec (literal "/usr/bin/security"))'

if [ "$(uname -s)" != Darwin ]; then
    echo "keychain-fence: the fence is macOS only; cannot make this claim on this host" >&2
    exit 2
fi
command -v sandbox-exec >/dev/null 2>&1 || {
    echo "keychain-fence: sandbox-exec unavailable; cannot fence the keychain" >&2
    exit 2
}

if [ -n "${ETUDE_JOURNAL_KEY:-}" ]; then
    echo "keychain-fence: ETUDE_JOURNAL_KEY is set in this shell; it would hide a test" >&2
    echo "                that forgets its key. Unset it and run again." >&2
    exit 2
fi

# ---------------------------------------------------------------- step 1 of 3
echo "==> proving the fence denies a program from starting"
# The fence must let an ordinary program run...
if ! sandbox-exec -p "$FENCE" /usr/bin/true; then
    echo "keychain-fence: a harmless program failed inside the fence" >&2
    exit 2
fi
# ...and the same kind of rule must stop the program it names. The control
# denies /usr/bin/true, and the failure must be the denial, not something else.
denied=$(sandbox-exec -p "$FENCE"'(deny process-exec (literal "/usr/bin/true"))' /usr/bin/true 2>&1) && {
    echo "keychain-fence: a denied program still started. the profile is broken" >&2
    exit 2
}
case "$denied" in
    *execvp*"Operation not permitted"*) ;;
    *)
        echo "keychain-fence: the control failed for an unexpected reason: $denied" >&2
        exit 2
        ;;
esac
echo "    ok: a harmless program runs inside the fence, and is stopped when denied"

# ---------------------------------------------------------------- step 2 of 3
echo "==> building (outside the fence; building never needs the keychain)"
cargo build --tests --quiet

# ---------------------------------------------------------------- step 3 of 3
# Seatbelt hides child processes from a sandboxed observer, so the one
# process-control test cannot see its extractor inside any sandbox (the network
# check has the same exception). It runs outside the fence, keyed by the same
# helper as every other test, which scripts/check-test-keys.py enforces.
echo "==> running the suite inside the keychain fence (excluding the process-control test)"
sandbox-exec -p "$FENCE" cargo test --all --quiet "$@" -- \
    --skip sigterm_stops_extractor_cleans_staging_and_reports_final_result || {
    status=$?
    echo "FAIL: the suite failed inside the keychain fence." >&2
    exit "$status"
}
echo "==> running the process-control test outside the fence (it gives the tool a key)"
cargo test --quiet "$@" -p unpack-cli --test sigterm -- \
    sigterm_stops_extractor_cleans_staging_and_reports_final_result || {
    status=$?
    echo "FAIL: the process-control test failed." >&2
    exit "$status"
}

echo
echo "PASS: the suite ran where the login keychain cannot be reached."
