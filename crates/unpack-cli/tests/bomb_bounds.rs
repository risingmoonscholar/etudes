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
fn an_archive_that_writes_past_the_cap_is_stopped_and_leaves_nothing() {
    let d = work("over");
    let _g = TestDir(d.clone());

    // 2100 MB > MAX_MEMBER_BYTES (2 GB). Building it takes a moment and a
    // couple of GB of scratch; skipping beats asserting nothing on a host
    // that cannot spare the room.
    let Some(bomb) = zip_expanding_to(&d, 2100) else {
        eprintln!("skipped: could not build the fixture (no zip, or no room)");
        return;
    };

    let out = d.join("out");
    let r = unpack_bin()
        .arg(&bomb)
        .arg("--into")
        .arg(&out)
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
        stderr.contains("cap"),
        "the refusal should say a cap was hit, not just fail: {stderr}"
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
