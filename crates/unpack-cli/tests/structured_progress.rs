//! Unpack's progress is observed, not parsed. What it says about a real
//! extraction is checked against the tree the extractor really produced.
include!("../../etude-cli-support/test-support/progress_probe.rs");

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Output, Stdio};

const UNPACK: &str = env!("CARGO_BIN_EXE_unpack");

fn tool(program: &str, args: &[&str], cwd: &Path) {
    let status = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap();
    assert!(status.success(), "{program} {args:?}");
}

fn tree(dir: &ProbeDir, files: usize) {
    let source = dir.join("source");
    for n in 0..files {
        let folder = source.join(format!("deep/er/{}", n % 3));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(format!("file {n}.txt")), b"synthetic").unwrap();
    }
}

fn unpack(args: &[&str]) -> Output {
    Command::new(UNPACK).args(args).output().unwrap()
}

fn records(stderr: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stderr)
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(str::to_string)
        .collect()
}

#[test]
fn every_format_reports_exactly_the_members_it_listed_and_implicit_parents_add_nothing() {
    for (suffix, flags) in [
        ("tar", "-cf"),
        ("tar.gz", "-czf"),
        ("tgz", "-czf"),
        ("tar.bz2", "-cjf"),
        ("tar.xz", "-cJf"),
    ] {
        let dir = ProbeDir::new("formats");
        tree(&dir, 6);
        let archive = dir.join(&format!("a.{suffix}"));
        // Files only: deep/, deep/er/ and the numbered folders are never
        // listed, so a count of the tree would read 11 against a plan of 6.
        let mut names: Vec<String> = (0..6)
            .map(|n| format!("deep/er/{}/file {n}.txt", n % 3))
            .collect();
        let mut args = vec![flags.to_string(), archive.display().to_string()];
        args.append(&mut names);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        tool("/usr/bin/tar", &args, &dir.join("source"));
        let output = dir.join("out");
        let run = unpack(&[
            archive.to_str().unwrap(),
            "--into",
            output.to_str().unwrap(),
            "--json",
            "--progress-json",
        ]);
        assert_eq!(run.status.code(), Some(0), "{suffix}");
        witness(
            "extract",
            &run.stdout,
            &run.stderr,
            Expect {
                planned: Some(6),
                completed: Some(6),
                journalled: Some(0),
                forbid: &["file 0", "deep/er", dir.0.to_str().unwrap()],
                ..Default::default()
            },
        );
    }
}

#[test]
fn a_zip_counts_its_listed_directories_and_files() {
    let dir = ProbeDir::new("zip");
    tree(&dir, 5);
    tool(
        "/usr/bin/zip",
        &["-qr", "../a.zip", "deep"],
        &dir.join("source"),
    );
    let archive = dir.join("a.zip");
    let listed = unpack(&[archive.to_str().unwrap(), "--list", "--progress-json"]);
    // 5 files, plus deep/, deep/er/ and three numbered folders.
    witness(
        "list",
        &listed.stdout,
        &listed.stderr,
        Expect {
            planned: Some(10),
            completed: Some(10),
            ..Default::default()
        },
    );
    let run = unpack(&[
        archive.to_str().unwrap(),
        "--into",
        dir.join("out").to_str().unwrap(),
        "--json",
        "--progress-json",
    ]);
    witness(
        "extract",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(10),
            completed: Some(10),
            ..Default::default()
        },
    );
}

#[test]
fn metadata_this_tool_drops_is_not_planned_or_counted() {
    let dir = ProbeDir::new("junk");
    let source = dir.join("source");
    std::fs::create_dir_all(source.join("__MACOSX")).unwrap();
    std::fs::write(source.join("keep.txt"), b"x").unwrap();
    std::fs::write(source.join(".DS_Store"), b"x").unwrap();
    std::fs::write(source.join("__MACOSX/._keep.txt"), b"x").unwrap();
    tool(
        "/usr/bin/tar",
        &[
            "-cf",
            "../a.tar",
            "keep.txt",
            ".DS_Store",
            "__MACOSX/._keep.txt",
        ],
        &source,
    );
    let run = unpack(&[
        dir.join("a.tar").to_str().unwrap(),
        "--into",
        dir.join("out").to_str().unwrap(),
        "--json",
        "--progress-json",
    ]);
    witness(
        "extract",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(1),
            completed: Some(1),
            ..Default::default()
        },
    );
}

#[test]
fn a_duplicate_member_is_planned_once() {
    let dir = ProbeDir::new("dup");
    // Written by hand so the same name is two plain file entries on every
    // machine. `tar -cf` stores the second copy of one file as a hard link
    // on GNU tar and as a second file on bsdtar, and unpack rightly refuses
    // the hard link.
    let writer = r#"
import io, sys, tarfile
with tarfile.open(sys.argv[1], "w") as t:
    for name in ("same.txt", "other.txt", "same.txt"):
        info = tarfile.TarInfo(name)
        info.size = 1
        t.addfile(info, io.BytesIO(b"x"))
"#;
    tool(
        "python3",
        &["-I", "-c", writer, dir.join("a.tar").to_str().unwrap()],
        &dir.0,
    );
    let run = unpack(&[
        dir.join("a.tar").to_str().unwrap(),
        "--into",
        dir.join("out").to_str().unwrap(),
        "--json",
        "--progress-json",
    ]);
    witness(
        "extract",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(2),
            completed: Some(2),
            ..Default::default()
        },
    );
}

#[test]
fn a_member_stored_as_a_hard_link_is_refused_before_anything_is_counted() {
    let dir = ProbeDir::new("hardlink");
    let writer = r#"
import io, sys, tarfile
with tarfile.open(sys.argv[1], "w") as t:
    info = tarfile.TarInfo("same.txt")
    info.size = 1
    t.addfile(info, io.BytesIO(b"x"))
    link = tarfile.TarInfo("same.txt")
    link.type = tarfile.LNKTYPE
    link.linkname = "same.txt"
    t.addfile(link)
"#;
    let archive = dir.join("a.tar");
    tool(
        "python3",
        &["-I", "-c", writer, archive.to_str().unwrap()],
        &dir.0,
    );
    let run = unpack(&[
        archive.to_str().unwrap(),
        "--into",
        dir.join("out").to_str().unwrap(),
        "--json",
        "--progress-json",
    ]);
    assert_eq!(run.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&run.stdout).contains("\"unsafe_input\""));
    witness(
        "extract",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(0),
            completed: Some(0),
            ..Default::default()
        },
    );
    assert!(!dir.join("out").exists());
}

#[test]
fn a_name_the_extractor_cannot_produce_is_not_reported_as_seen() {
    // Case folding leaves one file on a case-insensitive volume and two on a
    // case-sensitive one. Either way the count is what the tree really holds.
    let dir = ProbeDir::new("case");
    let inner = dir.join("names");
    std::fs::create_dir_all(&inner).unwrap();
    let writer = r#"
import io, sys, tarfile
with tarfile.open(sys.argv[1], "w") as t:
    for name in ("Readme.txt", "README.TXT", "other.txt"):
        info = tarfile.TarInfo(name)
        info.size = 1
        t.addfile(info, io.BytesIO(b"x"))
"#;
    let archive = dir.join("a.tar");
    tool(
        "python3",
        &["-I", "-c", writer, archive.to_str().unwrap()],
        &inner,
    );
    let output = dir.join("out");
    let run = unpack(&[
        archive.to_str().unwrap(),
        "--into",
        output.to_str().unwrap(),
        "--json",
        "--progress-json",
    ]);
    assert_eq!(run.status.code(), Some(0));
    let present = std::fs::read_dir(&output)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            ["Readme.txt", "README.TXT", "other.txt"]
                .contains(&entry.file_name().to_str().unwrap_or(""))
        })
        .count();
    witness(
        "extract",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(3),
            completed: Some(present),
            ..Default::default()
        },
    );
}

#[test]
fn a_bare_gzip_is_one_member_and_not_complete_while_it_is_still_being_written() {
    // 400 MiB of zeros takes real time to write and compresses to almost
    // nothing. Unpack creates the output before the extractor starts, so a
    // count of what exists would say 1/1 at the first poll, with no bytes yet.
    const BYTES: u64 = 400 * 1024 * 1024;
    let dir = ProbeDir::new("gz");
    let archive = dir.join("big.bin.gz");
    let script = format!(
        "head -c {BYTES} /dev/zero | gzip -1 > '{}'",
        archive.display()
    );
    tool("/bin/sh", &["-c", &script], &dir.0);
    let mut child = Command::new(UNPACK)
        .args([
            archive.to_str().unwrap(),
            "--into",
            dir.join("out").to_str().unwrap(),
            "--progress-json",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut saw_complete_record = false;
    let mut lines = Vec::new();
    for line in BufReader::new(child.stderr.take().unwrap()).lines() {
        let line = line.unwrap();
        // A record that says the member is finished is read while staging still
        // exists, so the bytes can be checked right then.
        if line.contains("\"phase\":\"advancing\"") && line.contains("\"completed\":1") {
            saw_complete_record = true;
            let member = std::fs::read_dir(&dir.0)
                .unwrap()
                .filter_map(Result::ok)
                .find(|entry| entry.file_name().to_string_lossy().contains(".partial"))
                .map(|staging| staging.path().join("big.bin"))
                .or_else(|| Some(dir.join("out/big.bin")))
                .unwrap();
            let size = std::fs::metadata(&member).map(|m| m.len()).unwrap_or(0);
            assert_eq!(size, BYTES, "1/1 was reported over a partly written member");
        }
        lines.push(line);
    }
    assert!(child.wait().unwrap().success());
    assert!(saw_complete_record, "{lines:?}");
    witness(
        "extract",
        b"",
        lines.join("\n").as_bytes(),
        Expect {
            planned: Some(1),
            completed: Some(1),
            ..Default::default()
        },
    );
}

#[test]
fn stdout_and_the_result_keep_their_shape_with_the_flag() {
    let dir = ProbeDir::new("clean");
    tree(&dir, 3);
    tool(
        "/usr/bin/zip",
        &["-qr", "../a.zip", "deep"],
        &dir.join("source"),
    );
    let archive = dir.join("a.zip");
    let plain = |flag: &[&str], out: &str| {
        let mut args = vec![archive.to_str().unwrap().to_string()];
        args.push("--into".into());
        args.push(dir.join(out).display().to_string());
        args.extend(flag.iter().map(|s| s.to_string()));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        unpack(&args)
    };
    // Same destination spelling in both, so stdout can be compared byte for byte.
    let without = plain(&[], "out");
    std::fs::remove_dir_all(dir.join("out")).unwrap();
    let with = plain(&["--progress-json"], "out");
    assert_eq!(without.stdout, with.stdout, "human stdout changed");
    assert!(without.stderr.is_empty(), "small work stays quiet");

    let shape = |run: &Output| -> String {
        let script = r#"
import json, sys
value = json.load(sys.stdin)
categories = sorted(row["category"] for row in value["observations"]["categories"])
counters = {row["name"]: row["count"] for row in value["effects"]["counters"]}
print(json.dumps([sorted(value), value["status"], value["reason"], value["scope"], categories, counters, value["details"]], sort_keys=True))
"#;
        let mut parser = Command::new("python3")
            .args(["-I", "-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(parser.stdin.as_mut().unwrap(), &run.stdout).unwrap();
        let output = parser.wait_with_output().unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap()
    };
    std::fs::remove_dir_all(dir.join("out")).unwrap();
    let without = plain(&["--json"], "out");
    std::fs::remove_dir_all(dir.join("out")).unwrap();
    let with = plain(&["--json", "--progress-json"], "out");
    assert_eq!(shape(&without), shape(&with), "the result changed");
}

#[test]
fn an_unsafe_archive_ends_in_error_without_ever_stating_a_count() {
    let dir = ProbeDir::new("unsafe");
    let writer = r#"
import io, sys, tarfile
with tarfile.open(sys.argv[1], "w") as t:
    info = tarfile.TarInfo("../escape.txt")
    info.size = 1
    t.addfile(info, io.BytesIO(b"x"))
"#;
    let archive = dir.join("a.tar");
    tool(
        "python3",
        &["-I", "-c", writer, archive.to_str().unwrap()],
        &dir.0,
    );
    let run = unpack(&[archive.to_str().unwrap(), "--json", "--progress-json"]);
    assert_eq!(run.status.code(), Some(2));
    witness(
        "extract",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(0),
            completed: Some(0),
            forbid: &["escape"],
            ..Default::default()
        },
    );
    let rows = records(&run.stderr);
    assert!(rows.last().unwrap().contains("\"phase\":\"error\""));
}

#[test]
fn work_that_never_starts_writes_no_records() {
    let dir = ProbeDir::new("refused");
    let run = unpack(&[dir.join("absent.zip").to_str().unwrap(), "--progress-json"]);
    assert_eq!(run.status.code(), Some(3));
    assert!(records(&run.stderr).is_empty());
    std::fs::write(dir.join("image.dmg"), b"x").unwrap();
    let run = unpack(&[dir.join("image.dmg").to_str().unwrap(), "--progress-json"]);
    assert_eq!(run.status.code(), Some(2));
    assert!(records(&run.stderr).is_empty());
}

#[test]
fn an_unknown_option_names_the_progress_flag_among_the_ones_that_work() {
    let dir = ProbeDir::new("unknown-option");
    let run = unpack(&[dir.join("a.zip").to_str().unwrap(), "--frob"]);
    assert_eq!(run.status.code(), Some(2));
    let text = String::from_utf8_lossy(&run.stderr);
    assert!(text.contains("--progress-json"), "{text}");
}

#[test]
fn a_destination_that_exists_is_found_after_work_began_so_it_ends_in_error() {
    let dir = ProbeDir::new("destination-exists");
    tree(&dir, 1);
    tool(
        "/usr/bin/zip",
        &["-qr", "../a.zip", "deep"],
        &dir.join("source"),
    );
    std::fs::create_dir(dir.join("out")).unwrap();
    let run = unpack(&[
        dir.join("a.zip").to_str().unwrap(),
        "--into",
        dir.join("out").to_str().unwrap(),
        "--progress-json",
    ]);
    assert_eq!(run.status.code(), Some(2));
    let records = records(&run.stderr);
    assert_eq!(records.len(), 2, "{records:?}");
    assert!(records[0].contains("\"phase\":\"planning\""), "{records:?}");
    assert!(records[0].contains("\"planned\":0"), "{records:?}");
    assert!(records[1].contains("\"phase\":\"error\""), "{records:?}");
}

#[test]
fn a_garbage_archive_stays_indeterminate_then_fails() {
    let dir = ProbeDir::new("garbage");
    std::fs::write(dir.join("a.zip"), b"this is not a zip").unwrap();
    let run = unpack(&[
        dir.join("a.zip").to_str().unwrap(),
        "--json",
        "--progress-json",
    ]);
    assert_ne!(run.status.code(), Some(0));
    witness(
        "extract",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(0),
            completed: Some(0),
            ..Default::default()
        },
    );
    let rows = records(&run.stderr);
    assert_eq!(rows.len(), 2, "{rows:?}");
}

#[test]
fn an_empty_tar_is_done_with_zero() {
    let dir = ProbeDir::new("empty");
    let archive = dir.join("a.tar");
    std::fs::write(&archive, vec![0_u8; 10240]).unwrap();
    let run = unpack(&[archive.to_str().unwrap(), "--json", "--progress-json"]);
    assert_eq!(run.status.code(), Some(1));
    witness(
        "extract",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(0),
            completed: Some(0),
            ..Default::default()
        },
    );
    assert!(
        records(&run.stderr)
            .last()
            .unwrap()
            .contains("\"phase\":\"done\"")
    );
}

fn slow_zip(dir: &ProbeDir, members: usize) -> std::path::PathBuf {
    // Stored, one MiB each: extraction takes several polls on any host.
    let source = dir.join("source");
    std::fs::create_dir_all(&source).unwrap();
    let chunk = vec![7_u8; 1024 * 1024];
    let mut names = Vec::new();
    for n in 0..members {
        let name = format!("chunk{n}.bin");
        std::fs::write(source.join(&name), &chunk).unwrap();
        names.push(name);
    }
    let mut command = Command::new("/usr/bin/zip");
    command
        .args(["-0", "-q", "../a.zip"])
        .args(&names)
        .current_dir(&source);
    assert!(command.status().unwrap().success());
    dir.join("a.zip")
}

/// Run until a record shows extraction under way, then signal, and return every
/// line unpack wrote plus how many complete members staging held at that moment.
fn interrupt(dir: &ProbeDir, archive: &Path, signal: &str) -> (Vec<String>, usize, Option<i32>) {
    let mut child = Command::new(UNPACK)
        .args([
            archive.to_str().unwrap(),
            "--into",
            dir.join("out").to_str().unwrap(),
            "--progress-json",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stderr.take().unwrap());
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut complete = None;
    while reader.read_line(&mut line).unwrap() > 0 {
        let seen = line.trim_end().to_string();
        line.clear();
        let under_way = seen.contains("\"phase\":\"planning\"") && !seen.contains("\"planned\":0");
        lines.push(seen);
        if under_way {
            std::thread::sleep(std::time::Duration::from_millis(250));
            complete = Some(whole_members(dir));
            tool("/bin/kill", &[signal, &child.id().to_string()], &dir.0);
            break;
        }
    }
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut reader, &mut rest).unwrap();
    lines.extend(rest.lines().map(str::to_string));
    let status = child.wait().unwrap();
    (lines, complete.expect("extraction started"), status.code())
}

fn whole_members(dir: &ProbeDir) -> usize {
    let Some(staging) = std::fs::read_dir(&dir.0)
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.file_name().to_string_lossy().contains(".partial"))
    else {
        return usize::MAX;
    };
    std::fs::read_dir(staging.path())
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.metadata().is_ok_and(|m| m.len() == 1024 * 1024))
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn a_sigkill_leaves_whole_records_no_terminal_and_no_overstatement() {
    let dir = ProbeDir::new("kill");
    let archive = slow_zip(&dir, 300);
    let (lines, whole, code) = interrupt(&dir, &archive, "-KILL");
    assert_eq!(code, None, "killed by signal");
    witness(
        "extract",
        b"",
        lines.join("\n").as_bytes(),
        Expect {
            planned: Some(300),
            killed: true,
            ..Default::default()
        },
    );
    // Any record already written describes members complete by then.
    for row in &lines {
        if let Some(found) = row.split("\"completed\":").nth(1) {
            let completed: usize = found.split(',').next().unwrap().parse().unwrap();
            assert!(completed <= whole, "{completed} > {whole}: {row}");
        }
    }
    assert!(!dir.join("out").exists());
}

#[test]
fn a_sigterm_ends_in_an_error_with_the_last_observed_count() {
    let dir = ProbeDir::new("term");
    let archive = slow_zip(&dir, 300);
    let (lines, _, code) = interrupt(&dir, &archive, "-TERM");
    assert_eq!(code, Some(2));
    witness(
        "extract",
        b"",
        lines.join("\n").as_bytes(),
        Expect {
            planned: Some(300),
            ..Default::default()
        },
    );
    assert!(
        records(lines.join("\n").as_bytes())
            .last()
            .unwrap()
            .contains("\"phase\":\"error\"")
    );
}

/// macOS tar renames each file into place, so a walk during extraction meets
/// names that have just gone. Before the walk allowed for it, this archive
/// failed on every one of 20 runs. Five runs, because the failure was a race.
#[test]
fn a_tar_of_many_files_extracts_while_the_walk_races_its_renames() {
    let dir = ProbeDir::new("race");
    let source = dir.join("source");
    std::fs::create_dir_all(&source).unwrap();
    let mut names = Vec::new();
    for n in 0..5000 {
        let name = format!("f{n}");
        std::fs::File::create(source.join(&name)).unwrap();
        names.push(name);
    }
    std::fs::write(dir.join("names"), names.join("\n")).unwrap();
    tool(
        "/usr/bin/tar",
        &["-cf", "../a.tar", "-T", "../names"],
        &source,
    );
    for _ in 0..5 {
        let output = dir.join("out");
        let run = unpack(&[
            dir.join("a.tar").to_str().unwrap(),
            "--into",
            output.to_str().unwrap(),
        ]);
        assert_eq!(
            run.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert_eq!(std::fs::read_dir(&output).unwrap().count(), 5000);
        std::fs::remove_dir_all(&output).unwrap();
    }
}

#[test]
fn a_huge_listing_stays_inside_the_bound() {
    let dir = ProbeDir::new("bound");
    let source = dir.join("source");
    std::fs::create_dir_all(&source).unwrap();
    let mut names = Vec::new();
    for n in 0..5000 {
        let name = format!("f{n}");
        std::fs::File::create(source.join(&name)).unwrap();
        names.push(name);
    }
    std::fs::write(dir.join("names"), names.join("\n")).unwrap();
    tool(
        "/usr/bin/tar",
        &["-cf", "../a.tar", "-T", "../names"],
        &source,
    );
    let run = unpack(&[
        dir.join("a.tar").to_str().unwrap(),
        "--into",
        dir.join("out").to_str().unwrap(),
        "--json",
        "--progress-json",
    ]);
    witness(
        "extract",
        &run.stdout,
        &run.stderr,
        Expect {
            planned: Some(5000),
            completed: Some(5000),
            ..Default::default()
        },
    );
    // Without the flag a redirected run gets the shared human milestones.
    std::fs::remove_dir_all(dir.join("out")).unwrap();
    let run = unpack(&[
        dir.join("a.tar").to_str().unwrap(),
        "--into",
        dir.join("out").to_str().unwrap(),
    ]);
    let text = String::from_utf8_lossy(&run.stderr);
    assert!(text.lines().count() <= 11, "{text}");
    assert!(text.contains("unpack: 5000/5000 items"), "{text}");
    assert!(!String::from_utf8_lossy(&run.stdout).contains("items\n  "));
}
