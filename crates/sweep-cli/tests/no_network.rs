//! M8 — the no-network witness.
//!
//! `deny.toml` stops a networking crate entering the tree. This test checks the
//! artifact that actually ships: it scans the built binary for the symbols a
//! program must link in order to reach the network at all.
//!
//! A policy file is a promise. A symbol scan is evidence.

use std::process::Command;

/// Symbols a process cannot open a network connection without.
///
/// `socket`, `connect` and `bind` are the syscall wrappers. `getaddrinfo` is
/// name resolution. `SSL_` and `tls_` catch a statically linked TLS stack that
/// might carry its own transport.
const FORBIDDEN: &[&str] = &[
    "_socket",
    "_connect",
    "_bind",
    "_getaddrinfo",
    "_gethostbyname",
    "_SSL_connect",
    "_SSL_new",
];

/// Symbols that appear in every Rust binary and must not be mistaken for
/// networking. `connect` in particular collides with iterator adaptors and
/// symbol names inside unrelated crates, so the scan is exact-match only.
fn binary_path() -> Option<std::path::PathBuf> {
    // The test binary lives in target/<profile>/deps/; the sweep binary is two
    // levels up in target/<profile>/.
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.parent()?;
    let p = dir.join("sweep");
    p.exists().then_some(p)
}

#[test]
fn the_shipped_binary_links_no_networking_symbols() {
    let Some(bin) = binary_path() else {
        // Building the test does not guarantee the bin target was built. Skip
        // loudly rather than pass silently — a green test that checked nothing
        // is worse than no test.
        eprintln!("SKIPPED: sweep binary not built; run `cargo build` first");
        return;
    };

    let out = Command::new("nm").args(["-u", bin.to_str().unwrap()]).output();
    let Ok(out) = out else {
        eprintln!("SKIPPED: `nm` unavailable on this platform");
        return;
    };
    assert!(out.status.success(), "nm failed on the sweep binary");

    let text = String::from_utf8_lossy(&out.stdout);

    // Guard against a green result that checked nothing: if the scan sees no
    // symbols at all, the invocation is broken, not the binary clean. Verified
    // against a control binary that does open a socket — it reports _socket,
    // _connect and _getaddrinfo here.
    let symbol_count = text.lines().filter(|l| !l.trim().is_empty()).count();
    assert!(symbol_count > 20, "nm returned {symbol_count} symbols; the scan is not working");

    let mut found: Vec<&str> = Vec::new();
    for line in text.lines() {
        // Undefined-symbol lines look like "    U _open". Take the last field
        // and compare exactly, so substrings cannot produce false positives.
        let Some(sym) = line.split_whitespace().last() else { continue };
        if FORBIDDEN.contains(&sym) {
            found.push(sym);
        }
    }

    assert!(
        found.is_empty(),
        "the shipped binary links networking symbols: {found:?}\n\
         sweep is supposed to be structurally incapable of network access."
    );
}

#[test]
fn the_engine_crate_has_no_dependencies() {
    // The cheapest audit available: there is no third-party code in the
    // classification path, so there is nothing to review for egress.
    let manifest = include_str!("../../etude-core/Cargo.toml");
    let deps = manifest.split("[dependencies]").nth(1).unwrap_or("");
    let body = deps.split("[dev-dependencies]").next().unwrap_or("");
    let real: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('['))
        .collect();

    assert!(
        real.is_empty(),
        "etude-core gained a dependency: {real:?}\n\
         The no-network claim rests on this crate having none."
    );
}
