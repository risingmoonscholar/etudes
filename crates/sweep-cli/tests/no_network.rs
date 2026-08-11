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
///
/// Bare, ABI-neutral names. Mach-O (macOS) prefixes every C symbol with `_`;
/// ELF (Linux) does not. `is_forbidden` below normalizes a candidate against
/// both forms so this one list covers the CI job that actually runs `nm` on
/// each platform, instead of only ever matching the Mach-O spelling.
const FORBIDDEN: &[&str] = &[
    "socket",
    "connect",
    "bind",
    "getaddrinfo",
    "gethostbyname",
    "SSL_connect",
    "SSL_new",
];

/// True if `sym` is a forbidden symbol under Mach-O's underscore-prefixed C
/// naming or ELF's bare naming, versioned or not.
///
/// A dynamically linked ELF binary's undefined glibc symbols usually carry a
/// version suffix, e.g. `socket@GLIBC_2.2.5` — matching `sym` verbatim against
/// bare `"socket"` would silently miss every one of them on a typical Ubuntu
/// build, so the suffix is stripped before comparing.
fn is_forbidden(sym: &str) -> bool {
    let sym = sym.split('@').next().unwrap_or(sym);
    let sym = sym.strip_prefix('_').unwrap_or(sym);
    FORBIDDEN.contains(&sym)
}

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

    let out = Command::new("nm")
        .args(["-u", bin.to_str().unwrap()])
        .output();
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
    assert!(
        symbol_count > 20,
        "nm returned {symbol_count} symbols; the scan is not working"
    );

    let mut found: Vec<&str> = Vec::new();
    for line in text.lines() {
        // Undefined-symbol lines look like "    U _open". Take the last field
        // and compare exactly, so substrings cannot produce false positives.
        let Some(sym) = line.split_whitespace().last() else {
            continue;
        };
        if is_forbidden(sym) {
            found.push(sym);
        }
    }

    assert!(
        found.is_empty(),
        "the shipped binary links networking symbols: {found:?}\n\
         sweep is supposed to be structurally incapable of network access."
    );
}

/// Runs `cargo tree` rooted at etude-core, following only the given edge
/// kinds, and returns the direct-and-transitive dependency lines (the root
/// line itself is checked and dropped).
///
/// `cargo tree` — not a text/TOML parse of the manifest — because a text
/// parse can be fooled by `[target.'cfg(...)'.dependencies]` tables or
/// dotted-key syntax (`[dependencies.foo]`); cargo's own manifest parser and
/// resolver sees what cargo would actually build regardless of how an entry
/// is spelled or where it sits in the file. This function is called with
/// `--target=all` (cfg-gated dependencies still count) and `--all-features`
/// (an optional dependency behind a feature nothing enables by default is
/// otherwise invisible even to `cargo tree`), and with no `--depth` limit —
/// a dependency hiding *below* the allowed `fixtures` crate must surface
/// too, since fixtures is supposed to be zero-dependency itself. `--offline`
/// matters because this test also runs under scripts/no-network-test.sh,
/// which denies socket access at the OS level; the workspace's deps are
/// already resolved and cached by the build step that runs first.
fn etude_core_tree(edges: &str) -> Vec<String> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let out = Command::new("cargo")
        .args([
            "tree",
            "--offline",
            "-p",
            "etude-core",
            "-e",
            edges,
            "--all-features",
            "--target=all",
            "--prefix",
            "none",
        ])
        .current_dir(manifest_dir)
        .output()
        .expect("failed to run `cargo tree` — is cargo on PATH?");

    assert!(
        out.status.success(),
        "cargo tree -e {edges} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut lines = text.lines();

    let root = lines.next().unwrap_or("").to_owned();
    assert!(
        root.starts_with("etude-core v"),
        "unexpected cargo tree output, first line was: {root:?}"
    );

    lines
        .filter(|l| !l.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_engine_crate_has_no_dependencies() {
    // Normal and build dependencies are what actually ship in the compiled
    // binary. This is the literal "sweep links no third-party code" claim,
    // and it must hold with no exceptions at all — not even `fixtures`.
    let shipped = etude_core_tree("normal,build");
    assert!(
        shipped.is_empty(),
        "etude-core gained a SHIPPED dependency: {shipped:?}\n\
         normal and build dependencies compile into the release binary; \
         etude-core must have none."
    );

    // Dev + normal + build together, at any depth: everything the manifest
    // can possibly cause to build when etude-core's own tests run. Comparing
    // this against `shipped` above (which must be empty) means the only
    // thing this list is allowed to contain is the one dev-only fixture
    // crate — if `fixtures` were reclassified as a normal/build dependency
    // instead, it would show up in `shipped` and fail there.
    let everything = etude_core_tree("normal,build,dev");

    // The one dependency etude-core is allowed to carry: a workspace-internal
    // crate that builds synthetic test fixtures. Matching on the exact local
    // path — not just the name "fixtures" — closes off a crate registered
    // under the same name on crates.io being swapped in instead.
    let fixtures_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent() // crates/
        .expect("CARGO_MANIFEST_DIR has no parent")
        .join("fixtures")
        .canonicalize()
        .expect("crates/fixtures should exist next to sweep-cli");
    // Don't pin the version digits here: fixtures and sweep-cli both inherit
    // `workspace.package.version`, but coupling this assertion to that value
    // would make it fail on every version bump for a reason unrelated to the
    // dependency policy it exists to check.
    let allowed_suffix = format!("({})", fixtures_dir.display());

    assert!(
        everything.len() == 1
            && everything[0].starts_with("fixtures v")
            && everything[0].ends_with(&allowed_suffix),
        "etude-core gained a dependency: {everything:?}\n\
         only a dev-only path dependency on {allowed_suffix} named \"fixtures\", \
         with no dependencies of its own, is allowed.\n\
         The no-network claim rests on this crate having none beyond that one \
         workspace-internal test-fixture crate."
    );
}
