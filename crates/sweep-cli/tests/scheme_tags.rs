//! `sweep --scheme tags` at the command line: the plan is the gate, tags are
//! only added, nothing moves, and undo puts every file's tags back.
//!
//! Tag values are written and read through the system's `xattr` and `plutil`,
//! not through sweep's own code.

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const KEY: &str = "0202020202020202020202020202020202020202020202020202020202020202";

struct Scene {
    root: PathBuf,
    state: PathBuf,
    plan: PathBuf,
}

impl Drop for Scene {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.root.parent().unwrap());
    }
}

fn scene(label: &str) -> Scene {
    let base = std::env::temp_dir().join(format!(
        "sweep-scheme-tags-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let root = base.join("root");
    let state = base.join("state");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&state).unwrap();
    for n in 1..=3 {
        fs::write(root.join(format!("report-{n}.pdf")), format!("pdf {n}")).unwrap();
    }
    for n in 1..=2 {
        fs::write(root.join(format!("photo-{n}.png")), format!("png {n}")).unwrap();
    }
    Scene {
        root: root.canonicalize().unwrap(),
        plan: base.join("plan.bin"),
        state,
    }
}

fn sweep(scene: &Scene, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sweep"))
        .env("ETUDE_STATE_DIR", &scene.state)
        .env("ETUDE_JOURNAL_KEY", KEY)
        .stdin(std::process::Stdio::null())
        .args(args)
        .output()
        .unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn root(s: &Scene) -> &str {
    s.root.to_str().unwrap()
}

/// Export a tag plan for `--map` pairs and return its digest.
fn export(s: &Scene, maps: &[&str]) -> String {
    let mut args = vec![root(s), "--scheme", "tags", "--since", "0"];
    for map in maps {
        args.push("--map");
        args.push(map);
    }
    args.extend(["--export-plan", s.plan.to_str().unwrap(), "--json"]);
    let exported = sweep(s, &args);
    assert!(
        exported.status.success(),
        "{}{}",
        out(&exported),
        err(&exported)
    );
    out(&exported)
        .split("\"plan_digest\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("digest in the result")
        .to_string()
}

fn apply_plan(s: &Scene, digest: &str, extra: &[&str]) -> Output {
    let mut args = vec![
        "apply",
        "--plan",
        s.plan.to_str().unwrap(),
        "--plan-digest",
        digest,
        "--yes",
    ];
    args.extend(extra);
    sweep(s, &args)
}

#[cfg(target_os = "macos")]
mod on_macos {
    use super::*;
    use std::io::Write;
    use std::process::Stdio;

    const TAG: &str = "com.apple.metadata:_kMDItemUserTags";

    fn raw(path: &Path) -> Option<Vec<u8>> {
        let o = Command::new("xattr")
            .args(["-px", TAG])
            .arg(path)
            .output()
            .unwrap();
        o.status.success().then(|| {
            let digits: Vec<u8> = o
                .stdout
                .iter()
                .copied()
                .filter(u8::is_ascii_hexdigit)
                .collect();
            digits
                .chunks(2)
                .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
                .collect()
        })
    }

    fn set_tags(path: &Path, tags: &[&str]) {
        let json = format!(
            "[{}]",
            tags.iter()
                .map(|t| format!("\"{}\"", t.replace('\n', "\\n")))
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut child = Command::new("plutil")
            .args(["-convert", "binary1", "-o", "-", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(json.as_bytes())
            .unwrap();
        let bytes = child.wait_with_output().unwrap().stdout;
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        assert!(
            Command::new("xattr")
                .args(["-wx", TAG, &hex])
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
    }

    fn tags_of(path: &Path) -> Option<Vec<String>> {
        let bytes = raw(path)?;
        let mut child = Command::new("plutil")
            .args(["-convert", "json", "-o", "-", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&bytes).unwrap();
        let text = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        let inner = text.trim().trim_start_matches('[').trim_end_matches(']');
        Some(
            inner
                .split("\",\"")
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_matches('"').replace("\\n", "\n"))
                .collect(),
        )
    }

    /// Names, contents, tag bytes and times of every file under the root.
    fn tree(root: &Path) -> Vec<String> {
        use std::os::unix::fs::MetadataExt;
        let mut rows: Vec<String> = fs::read_dir(root)
            .unwrap()
            .flatten()
            .map(|e| {
                let m = fs::symlink_metadata(e.path()).unwrap();
                format!(
                    "{} {:?} {:?} {} {} {}",
                    e.file_name().to_string_lossy(),
                    fs::read(e.path()).ok(),
                    raw(&e.path()),
                    m.mtime_nsec(),
                    m.ctime_nsec(),
                    m.ino()
                )
            })
            .collect();
        rows.sort();
        rows
    }

    #[test]
    fn the_plan_names_each_tag_with_its_count_and_marks_the_new_ones() {
        let s = scene("plan");
        set_tags(&s.root.join("photo-1.png"), &["Receipts\n4"]);
        let before = tree(&s.root);
        let human = sweep(
            &s,
            &[
                root(&s),
                "--scheme",
                "tags",
                "--map",
                "pdf=Work",
                "--map",
                "png=Receipts",
                "--since",
                "0",
            ],
        );
        assert!(human.status.success(), "{}{}", out(&human), err(&human));
        let text = out(&human);
        assert!(text.contains("Tags to add"), "{text}");
        assert!(
            text.lines()
                .any(|l| l.contains("Work") && l.contains("3 files") && l.contains("NEW TAG")),
            "{text}"
        );
        assert!(
            text.lines()
                .any(|l| l.contains("Receipts") && l.contains("1 file") && !l.contains("NEW TAG")),
            "{text}"
        );
        assert!(text.contains("1 tag is new: Work"), "{text}");
        assert!(
            text.contains("Finder's own tag list cannot be read"),
            "{text}"
        );
        assert!(text.contains("Nothing has been tagged or moved"), "{text}");
        assert!(
            text.contains("Nothing is moved, renamed, created or deleted"),
            "{text}"
        );
        assert_eq!(
            tree(&s.root),
            before,
            "planning wrote nothing and touched nothing"
        );

        let json = out(&sweep(
            &s,
            &[
                root(&s),
                "--scheme",
                "tags",
                "--map",
                "pdf=Work",
                "--map",
                "png=Receipts",
                "--since",
                "0",
                "--json",
            ],
        ));
        assert!(json.contains("\"scheme\":\"tags\""), "{json}");
        assert!(json.contains("\"new_tags\":[\"Work\"]"), "{json}");
        assert!(
            json.contains("{\"name\":\"Work\",\"files\":3,\"new\":true,\"extensions\":[\"pdf\"]}"),
            "{json}"
        );
        assert!(
            json.contains(
                "{\"name\":\"Receipts\",\"files\":1,\"new\":false,\"extensions\":[\"png\"]}"
            ),
            "{json}"
        );
        assert_eq!(tree(&s.root), before);
    }

    #[test]
    fn nothing_is_written_until_a_plan_the_person_has_seen_is_applied() {
        let s = scene("gate");
        let before = tree(&s.root);
        // Exporting a plan writes nothing to the tree.
        let digest = export(&s, &["pdf=Work"]);
        assert_eq!(tree(&s.root), before);
        // The live path, which plans and applies in one step, is closed to tags.
        let live = sweep(
            &s,
            &[
                "apply",
                root(&s),
                "--scheme",
                "tags",
                "--map",
                "pdf=Work",
                "--yes",
                "--since",
                "0",
            ],
        );
        assert_eq!(live.status.code(), Some(2), "{}{}", out(&live), err(&live));
        assert!(err(&live).contains("exported plan"), "{}", err(&live));
        // Review moves files and takes no tags.
        let review = sweep(
            &s,
            &[
                "review",
                root(&s),
                "--scheme",
                "tags",
                "--map",
                "pdf=Work",
                "--since",
                "0",
            ],
        );
        assert_eq!(review.status.code(), Some(2));
        // A wrong digest is refused and writes nothing.
        let wrong = apply_plan(&s, &"0".repeat(64), &[]);
        assert!(!wrong.status.success());
        // A plan applies only with an explicit yes.
        let no_yes = sweep(
            &s,
            &[
                "apply",
                "--plan",
                s.plan.to_str().unwrap(),
                "--plan-digest",
                &digest,
            ],
        );
        assert_eq!(no_yes.status.code(), Some(2));
        assert_eq!(tree(&s.root), before, "still nothing written");
        // The one way in writes the tags.
        let applied = apply_plan(&s, &digest, &[]);
        assert!(
            applied.status.success(),
            "{}{}",
            out(&applied),
            err(&applied)
        );
        assert_eq!(tags_of(&s.root.join("report-1.pdf")).unwrap(), ["Work"]);
    }

    #[test]
    fn apply_adds_only_moves_nothing_and_undo_restores_every_previous_set() {
        let s = scene("roundtrip");
        set_tags(&s.root.join("report-1.pdf"), &["Red\n6", "Old"]);
        set_tags(&s.root.join("report-2.pdf"), &["Mine"]);
        let before = tree(&s.root);
        let names_before: Vec<String> = fs::read_dir(&s.root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let raw_before: Vec<_> = ["report-1.pdf", "report-2.pdf", "report-3.pdf"]
            .iter()
            .map(|n| raw(&s.root.join(n)))
            .collect();

        let digest = export(&s, &["pdf=Work"]);
        let applied = apply_plan(&s, &digest, &[]);
        let text = out(&applied);
        assert!(applied.status.success(), "{text}{}", err(&applied));
        assert!(text.contains("Tagged 3 files"), "{text}");
        assert!(text.contains("Nothing was moved"), "{text}");
        assert_eq!(
            tags_of(&s.root.join("report-1.pdf")).unwrap(),
            ["Red\n6", "Old", "Work"]
        );
        assert_eq!(
            tags_of(&s.root.join("report-2.pdf")).unwrap(),
            ["Mine", "Work"]
        );
        assert_eq!(tags_of(&s.root.join("report-3.pdf")).unwrap(), ["Work"]);
        // Same names, no new folder, photos untouched.
        let mut names_after: Vec<String> = fs::read_dir(&s.root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let mut expected = names_before.clone();
        names_after.sort();
        expected.sort();
        assert_eq!(names_after, expected);
        assert_eq!(raw(&s.root.join("photo-1.png")), None);

        // Status works on a tag operation and counts the files changed.
        let status = sweep(&s, &["status", root(&s), "--json"]);
        assert!(status.status.success(), "{}{}", out(&status), err(&status));
        assert!(
            out(&status).contains("\"state\":\"completed\""),
            "{}",
            out(&status)
        );
        assert!(out(&status).contains("\"moved\":3"), "{}", out(&status));

        let undone = sweep(&s, &["undo", root(&s)]);
        let text = out(&undone);
        assert!(undone.status.success(), "{text}{}", err(&undone));
        assert!(
            text.contains("Restored the previous tags on 3 files. Nothing was moved."),
            "{text}"
        );
        let raw_after: Vec<_> = ["report-1.pdf", "report-2.pdf", "report-3.pdf"]
            .iter()
            .map(|n| raw(&s.root.join(n)))
            .collect();
        assert_eq!(raw_after, raw_before, "the exact previous bytes");
        assert_eq!(tree(&s.root).len(), before.len());
        let status = sweep(&s, &["status", root(&s), "--json"]);
        assert!(
            out(&status).contains("\"state\":\"reversed\""),
            "{}",
            out(&status)
        );
        // A second undo has nothing left to do.
        let again = sweep(&s, &["undo", root(&s)]);
        assert_eq!(again.status.code(), Some(1));
    }

    #[test]
    fn undo_names_a_file_whose_tags_changed_and_leaves_it() {
        let s = scene("changed");
        let digest = export(&s, &["pdf=Work"]);
        assert!(apply_plan(&s, &digest, &[]).status.success());
        set_tags(&s.root.join("report-2.pdf"), &["Work", "Added By Hand"]);
        let undone = sweep(&s, &["undo", root(&s)]);
        let text = out(&undone);
        assert!(undone.status.success(), "{text}{}", err(&undone));
        assert!(
            text.contains("Restored the previous tags on 2 files"),
            "{text}"
        );
        assert!(
            text.contains("1 file had its tags changed since apply and was left alone"),
            "{text}"
        );
        assert!(text.contains(".pdf>"), "{text}");
        assert_eq!(
            tags_of(&s.root.join("report-2.pdf")).unwrap(),
            ["Work", "Added By Hand"]
        );
        assert_eq!(raw(&s.root.join("report-1.pdf")), None);
        // Undoing again repeats the refusal and still overwrites nothing.
        let again = sweep(&s, &["undo", root(&s)]);
        assert!(out(&again).contains("left alone"), "{}", out(&again));
        assert_eq!(
            tags_of(&s.root.join("report-2.pdf")).unwrap(),
            ["Work", "Added By Hand"]
        );
    }

    #[test]
    fn a_plan_made_before_the_tags_changed_is_stale_and_writes_nothing() {
        let s = scene("stale");
        set_tags(&s.root.join("report-1.pdf"), &["Old"]);
        let digest = export(&s, &["pdf=Work"]);
        std::thread::sleep(std::time::Duration::from_millis(20));
        set_tags(&s.root.join("report-1.pdf"), &["Old", "Changed"]);
        let before = tree(&s.root);
        let applied = apply_plan(&s, &digest, &[]);
        assert_eq!(
            applied.status.code(),
            Some(2),
            "{}{}",
            out(&applied),
            err(&applied)
        );
        assert!(err(&applied).contains("replan"), "{}", err(&applied));
        assert_eq!(tree(&s.root), before);
    }

    #[test]
    fn a_file_with_no_readable_tag_state_is_listed_and_never_given_a_guess() {
        let s = scene("unreadable");
        let odd = s.root.join("report-2.pdf");
        assert!(
            Command::new("xattr")
                .args(["-w", TAG, "not a property list"])
                .arg(&odd)
                .status()
                .unwrap()
                .success()
        );
        let before = raw(&odd);
        let text = out(&sweep(
            &s,
            &[
                root(&s),
                "--scheme",
                "tags",
                "--map",
                "pdf=Work",
                "--since",
                "0",
            ],
        ));
        assert!(
            text.contains("1 file's tags could not be read; nothing will be added to it"),
            "{text}"
        );
        assert!(
            text.lines()
                .any(|l| l.contains("Work") && l.contains("2 files")),
            "{text}"
        );
        let json = out(&sweep(
            &s,
            &[
                root(&s),
                "--scheme",
                "tags",
                "--map",
                "pdf=Work",
                "--since",
                "0",
                "--json",
            ],
        ));
        assert!(json.contains("\"state_unreadable\":1"), "{json}");
        let digest = export(&s, &["pdf=Work"]);
        assert!(apply_plan(&s, &digest, &[]).status.success());
        assert_eq!(raw(&odd), before, "left exactly as found");
    }

    #[test]
    fn a_tagged_file_needs_no_include_tagged_and_the_flag_is_refused() {
        let s = scene("tagged");
        set_tags(&s.root.join("report-1.pdf"), &["Keep"]);
        let text = out(&sweep(
            &s,
            &[
                root(&s),
                "--scheme",
                "tags",
                "--map",
                "pdf=Work",
                "--since",
                "0",
            ],
        ));
        assert!(
            text.lines()
                .any(|l| l.contains("Work") && l.contains("3 files")),
            "{text}"
        );
        assert!(!text.contains("Finder-tagged item"), "{text}");
        let flag = sweep(
            &s,
            &[
                root(&s),
                "--scheme",
                "tags",
                "--map",
                "pdf=Work",
                "--include-tagged",
                "--since",
                "0",
            ],
        );
        assert_eq!(flag.status.code(), Some(2));
        assert!(
            err(&flag).contains("--include-tagged does nothing"),
            "{}",
            err(&flag)
        );
    }

    #[test]
    fn bad_requests_are_refused_by_name() {
        let s = scene("refusals");
        let before = tree(&s.root);
        let run = |extra: &[&str]| {
            let mut args = vec![root(&s), "--scheme", "tags", "--since", "0"];
            args.extend(extra);
            sweep(&s, &args)
        };
        let no_map = run(&[]);
        assert_eq!(no_map.status.code(), Some(2));
        assert!(
            err(&no_map).contains("needs at least one --map EXT=TAG"),
            "{}",
            err(&no_map)
        );
        for (map, why) in [
            ("pdf=", "cannot be empty"),
            ("pdf=a/b", "not a plain name"),
            ("pdf=two\nlines", "control character"),
            ("pdf=Work", ""),
        ] {
            let refused = run(&["--map", map, "--map", "pdf=Again"]);
            assert_eq!(refused.status.code(), Some(2), "{map}");
            assert!(
                err(&refused).contains(if why.is_empty() { "mapped twice" } else { why }),
                "{map}: {}",
                err(&refused)
            );
        }
        assert_eq!(tree(&s.root), before);
    }

    #[test]
    fn a_tag_plan_cannot_be_turned_into_folder_proposals_and_no_journal_apply_is_refused() {
        let s = scene("noproposal");
        let digest = export(&s, &["pdf=Work"]);
        let proposal = s.state.join("proposal.json");
        fs::write(
            &proposal,
            format!("{{\"plan_digest\":\"{digest}\",\"decisions\":[]}}"),
        )
        .unwrap();
        let out_plan = s.state.join("proposed.plan");
        let proposed = sweep(
            &s,
            &[
                "propose",
                "--plan",
                s.plan.to_str().unwrap(),
                "--plan-digest",
                &digest,
                "--proposal",
                proposal.to_str().unwrap(),
                "--export-plan",
                out_plan.to_str().unwrap(),
            ],
        );
        assert_eq!(
            proposed.status.code(),
            Some(2),
            "{}{}",
            out(&proposed),
            err(&proposed)
        );
        assert!(err(&proposed).contains("tags plan"), "{}", err(&proposed));
        assert!(!out_plan.exists());
        let before = tree(&s.root);
        let unjournaled = apply_plan(&s, &digest, &["--no-journal"]);
        assert_eq!(unjournaled.status.code(), Some(2));
        assert_eq!(tree(&s.root), before);
    }

    #[test]
    fn an_old_style_folder_plan_is_unaffected() {
        let s = scene("folders");
        let json = out(&sweep(&s, &[root(&s), "--since", "0", "--json"]));
        assert!(!json.contains("\"scheme\":\"tags\""), "{json}");
        assert!(!json.contains("new_tags"), "{json}");
    }
}

/// Finder tags are a macOS feature. Elsewhere the scheme is refused by name
/// and nothing is planned.
#[cfg(not(target_os = "macos"))]
#[test]
fn the_tags_scheme_is_refused_by_name_off_macos() {
    let s = scene("not-macos");
    let o = sweep(
        &s,
        &[
            root(&s),
            "--scheme",
            "tags",
            "--map",
            "pdf=Work",
            "--since",
            "0",
        ],
    );
    assert_eq!(o.status.code(), Some(2));
    assert!(err(&o).contains("needs macOS"), "{}", err(&o));
}
