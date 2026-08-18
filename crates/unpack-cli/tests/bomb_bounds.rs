//! What unpack writes is bounded, and the bound does not consult the archive.
//!
//! The measurement these exist for: editing four bytes makes `unzip -Z`,
//! `tar -tvf` and `gzip -l` each report 4,096 for a member that expands to
//! millions of bytes, and the forged zip then extracts in full with unzip
//! exiting 0 and printing nothing. Any check reading those numbers is a check
//! the attacker fills in.

use std::path::{Path, PathBuf};
use std::process::Command;

fn unpack_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_unpack"))
}

struct TestDir(PathBuf);
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn work(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("unpack_bomb_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

/// Build a zip whose single member expands to `mb` megabytes of zeroes.
fn zip_expanding_to(dir: &Path, mb: usize) -> Option<PathBuf> {
    let payload = dir.join("payload.bin");
    let chunk = vec![0u8; 1024 * 1024];
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&payload).ok()?;
        for _ in 0..mb {
            f.write_all(&chunk).ok()?;
        }
    }
    let zip = dir.join("bomb.zip");
    let ok = Command::new("zip")
        .arg("-q")
        .arg("-1")
        .arg("-j")
        .arg(&zip)
        .arg(&payload)
        .status()
        .ok()?
        .success();
    let _ = std::fs::remove_file(&payload);
    ok.then_some(zip)
}

#[test]
fn an_archive_that_writes_past_its_budget_is_stopped_and_leaves_nothing() {
    let d = work("over");
    let _g = TestDir(d.clone());

    // The budget is half the volume's free space, so a fixture big enough to
    // exceed it naturally would be enormous and would depend on the host. The
    // bound is what is under test, not the number, so `--max-size` sets a
    // small one and a 300 MB archive crosses it.
    let Some(bomb) = zip_expanding_to(&d, 300) else {
        eprintln!("skipped: could not build the fixture (no zip, or no room)");
        return;
    };

    let out = d.join("out");
    let r = unpack_bin()
        .arg(&bomb)
        .arg("--into")
        .arg(&out)
        .args(["--max-size", "64M"])
        .output()
        .expect("run unpack");

    assert_eq!(
        r.status.code(),
        Some(2),
        "a bomb must be refused (exit 2), got {:?}. stderr={}",
        r.status.code(),
        String::from_utf8_lossy(&r.stderr)
    );
    assert!(
        !out.exists(),
        "the partial extraction was left behind at {}",
        out.display()
    );
    let stderr = String::from_utf8_lossy(&r.stderr);
    assert!(
        stderr.contains("--max-size"),
        "a refusal must name the way past it, or it is a wall rather than a \
         safety feature: {stderr}"
    );
}

/// The default budget must not refuse an ordinary large archive.
///
/// The version this replaced used a fixed 4 GB total and 2 GB per member, so a
/// 6 GB project extracted from a 4 GB zip was refused on a machine with 800 GB
/// free -- and a big legitimate file was indistinguishable from a bomb by
/// construction. It still is indistinguishable; the difference is that the
/// bound now scales with the room available rather than with a guess.
#[test]
fn an_ordinary_large_archive_is_not_refused_by_default() {
    let d = work("large");
    let _g = TestDir(d.clone());

    let Some(zip) = zip_expanding_to(&d, 300) else {
        eprintln!("skipped: could not build the fixture");
        return;
    };

    let out = d.join("out");
    let r = unpack_bin()
        .arg(&zip)
        .arg("--into")
        .arg(&out)
        .output()
        .expect("run unpack");

    assert_eq!(
        r.status.code(),
        Some(0),
        "a 300 MB archive was refused by the default budget: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    assert!(
        walk_size(&out) > 200 * 1024 * 1024,
        "the archive did not actually extract"
    );
}

#[test]
fn a_forged_declared_size_does_not_help_an_archive_through() {
    let d = work("forged");
    let _g = TestDir(d.clone());

    let Some(zip) = zip_expanding_to(&d, 3) else {
        eprintln!("skipped: could not build the fixture");
        return;
    };

    // Rewrite the declared uncompressed size in both headers to 4096. A tool
    // that judged from the listing would now see a 4 KB member.
    let mut raw = std::fs::read(&zip).expect("read zip");
    for (sig, off) in [
        (b"PK\x01\x02".as_slice(), 24usize),
        (b"PK\x03\x04".as_slice(), 22),
    ] {
        let mut i = 0;
        while let Some(found) = raw[i..].windows(4).position(|w| w == sig).map(|p| p + i) {
            raw[found + off..found + off + 4].copy_from_slice(&4096u32.to_le_bytes());
            i = found + 4;
        }
    }
    std::fs::write(&zip, &raw).expect("write forged zip");

    let out = d.join("out");
    let r = unpack_bin()
        .arg(&zip)
        .arg("--into")
        .arg(&out)
        .output()
        .expect("run unpack");

    // 3 MB is under the cap, so it extracts -- the point is that the forged
    // number changed nothing either way. What must be true is that the bytes
    // on disk are the real ones, not the declared ones.
    assert_eq!(
        r.status.code(),
        Some(0),
        "a small archive should still extract"
    );
    let written: u64 = walk_size(&out);
    assert!(
        written > 2 * 1024 * 1024,
        "unpack wrote {written} bytes; it appears to have believed the forged 4096"
    );
}

fn walk_size(dir: &Path) -> u64 {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                n += walk_size(&e.path());
            } else {
                n += md.len();
            }
        }
    }
    n
}
