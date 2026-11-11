//! The steps printed under a plan, as a person reads and pastes them: the
//! in-use line sits with the Apply step, a scan that cannot apply prints
//! neither, and the Save step works when pasted from inside the folder or from
//! outside it, and so does the exact-plan command it prints.
#![cfg(target_os = "macos")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../../etude-cli-support/test-support/test_tool.rs"]
mod test_tool;
use test_tool::keyed_tool;

const IN_USE: &str =
    "sweep cannot tell if a file is still in use by a program. Check the plan before you apply it.";

struct Scene {
    base: PathBuf,
    root: PathBuf,
    home: PathBuf,
}

impl Drop for Scene {
    fn drop(&mut self) {
        let _ = Command::new("chflags")
            .args(["-R", "nouchg"])
            .arg(&self.base)
            .status();
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// Three PDFs in `inbox`, and a HOME of its own beside it.
fn scene(label: &str) -> Scene {
    let base =
        std::env::temp_dir().join(format!("sweep-next-steps-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let root = base.join("inbox");
    let home = base.join("home");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&home).unwrap();
    for name in ["a.pdf", "b.pdf", "c.pdf"] {
        fs::write(root.join(name), name).unwrap();
    }
    let base = base.canonicalize().unwrap();
    Scene {
        root: base.join("inbox"),
        home: base.join("home"),
        base,
    }
}

fn tools_path() -> String {
    // The test runs from target/<profile>/deps; the tools are one level up.
    let test = std::env::current_exe().unwrap();
    let bin = test.parent().and_then(Path::parent).unwrap();
    assert!(bin.join("sweep").is_file(), "no sweep in {}", bin.display());
    format!("{}:/usr/bin:/bin", bin.display())
}

fn sweep(scene: &Scene, cwd: &Path, args: &[&str]) -> Output {
    keyed_tool(env!("CARGO_BIN_EXE_sweep"))
        .env("HOME", &scene.home)
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}

/// Paste a printed line into a shell, the way a person would.
fn paste(scene: &Scene, cwd: &Path, line: &str) -> Output {
    keyed_tool("/bin/sh")
        .env("HOME", &scene.home)
        .env("PATH", tools_path())
        .current_dir(cwd)
        .arg("-c")
        .arg(line)
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn step<'a>(out: &'a str, label: &str) -> &'a str {
    out.lines()
        .find_map(|line| line.strip_prefix(label))
        .unwrap_or_else(|| panic!("no {label:?} line in:\n{out}"))
}

#[test]
fn the_in_use_line_comes_right_before_the_steps() {
    let s = scene("placement");
    let out = text(&sweep(
        &s,
        &s.base,
        &[s.root.to_str().unwrap(), "--since", "0"],
    ));
    let lines: Vec<&str> = out.lines().collect();
    let at = lines.iter().position(|l| *l == IN_USE).expect(&out);
    assert!(
        lines[..at]
            .iter()
            .any(|l| l.starts_with("Nothing has been moved."))
    );
    assert!(lines[at + 1].starts_with("Review: sweep review "), "{out}");
    assert_eq!(lines.iter().filter(|l| **l == IN_USE).count(), 1);
}

#[test]
fn quiet_keeps_the_in_use_line_with_the_apply_step() {
    let s = scene("quiet");
    let out = text(&sweep(
        &s,
        &s.base,
        &[s.root.to_str().unwrap(), "--since", "0", "--quiet"],
    ));
    assert!(
        out.contains(&format!("{IN_USE}\nReview: sweep review <path>")),
        "{out}"
    );
    assert!(
        out.contains("Apply:  sweep apply <path> --since 0 --yes"),
        "{out}"
    );
    assert!(!out.contains(s.root.to_str().unwrap()), "{out}");
}

#[test]
fn a_locked_folder_gets_no_in_use_line_and_no_steps() {
    let s = scene("locked");
    assert!(
        Command::new("chflags")
            .arg("uchg")
            .arg(&s.root)
            .status()
            .unwrap()
            .success()
    );
    let out = text(&sweep(
        &s,
        &s.base,
        &[s.root.to_str().unwrap(), "--since", "0"],
    ));
    assert!(out.contains("Not ready to apply"), "{out}");
    assert!(!out.contains(IN_USE), "{out}");
    assert!(!out.contains("Apply:"), "{out}");
    assert!(!out.contains("Save:"), "{out}");
}

#[test]
fn a_blocked_folder_gets_no_in_use_line_and_no_steps() {
    let s = scene("blocked");
    fs::write(s.root.join("Documents"), "a file where the folder would go").unwrap();
    let out = text(&sweep(
        &s,
        &s.base,
        &[s.root.to_str().unwrap(), "--since", "0"],
    ));
    assert!(out.contains("Not ready to apply"), "{out}");
    assert!(!out.contains(IN_USE), "{out}");
    assert!(!out.contains("Apply:"), "{out}");
    assert!(!out.contains("Save:"), "{out}");
}

/// Save, pasted as printed, then the exact-plan command it prints, pasted as
/// printed, moves the files. `plan` is the name the Save step must offer.
fn save_then_apply(s: &Scene, cwd: &Path, path: &str, plan: &str) {
    let out = text(&sweep(s, cwd, &[path, "--since", "0"]));
    let save = step(&out, "Save:   ");
    assert!(save.ends_with(&format!("--export-plan ~/{plan}")), "{out}");
    let saved = paste(s, cwd, save);
    let saved_text = text(&saved);
    assert!(saved.status.success(), "{save}\n{saved_text}");
    assert!(s.home.join(plan).is_file(), "{saved_text}");
    assert!(
        saved_text
            .contains("This moves only this plan. It refuses if anything it scanned has changed."),
        "{saved_text}"
    );
    let apply = step(&saved_text, "Apply this plan: ");
    assert!(!apply.contains("SHA"), "{apply}");
    let applied = paste(s, cwd, apply);
    assert!(applied.status.success(), "{apply}\n{}", text(&applied));
    assert!(
        s.root.join("Documents/a.pdf").is_file(),
        "{}",
        text(&applied)
    );
    assert!(!s.root.join("a.pdf").exists());
}

#[test]
fn the_save_line_pastes_from_inside_the_folder() {
    let s = scene("inside");
    save_then_apply(&s, &s.root, ".", "sweep.plan");
}

#[test]
fn the_save_line_pastes_from_outside_the_folder() {
    let s = scene("outside");
    save_then_apply(&s, &s.base, s.root.to_str().unwrap(), "sweep.plan");
}

/// The first plan file stays after its apply. The next scan's Save step must
/// offer a name that is free, or the paste fails with "already exists".
#[test]
fn the_save_line_pastes_a_second_time() {
    let s = scene("twice");
    save_then_apply(&s, &s.root, ".", "sweep.plan");
    for name in ["a.pdf", "b.pdf", "c.pdf"] {
        fs::rename(s.root.join("Documents").join(name), s.root.join(name)).unwrap();
    }
    fs::remove_dir(s.root.join("Documents")).unwrap();
    save_then_apply(&s, &s.root, ".", "sweep-2.plan");
    assert!(s.home.join("sweep.plan").is_file());
}

const TAG: &str = "com.apple.metadata:_kMDItemUserTags";

/// The tag names on a file, read back through `xattr` and `plutil`.
fn tags_of(path: &Path) -> Vec<String> {
    let hex = Command::new("xattr")
        .args(["-px", TAG])
        .arg(path)
        .output()
        .unwrap();
    if !hex.status.success() {
        return Vec::new();
    }
    let digits: Vec<u8> = hex
        .stdout
        .iter()
        .copied()
        .filter(u8::is_ascii_hexdigit)
        .collect();
    let bytes: Vec<u8> = digits
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect();
    let mut child = Command::new("plutil")
        .args(["-convert", "json", "-o", "-", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), &bytes).unwrap();
    let json = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
    json.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .filter(|t| !t.is_empty())
        .map(|t| t.trim_matches('"').to_owned())
        .collect()
}

fn names(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn export_tags(s: &Scene, plan: &Path) -> String {
    text(&sweep(
        s,
        &s.base,
        &[
            s.root.to_str().unwrap(),
            "--since",
            "0",
            "--scheme",
            "tags",
            "--map",
            "pdf=Receipts",
            "--export-plan",
            plan.to_str().unwrap(),
        ],
    ))
}

/// After an export, a tag plan prints the exact apply with the real digest,
/// says it adds tags and moves nothing, and that is what pasting it does.
#[test]
fn an_exported_tag_plan_prints_the_exact_apply_and_it_only_adds_tags() {
    let s = scene("tags");
    fs::write(s.root.join("notes.txt"), "not mapped").unwrap();
    let plan = s.home.join("tags.plan");
    let out = export_tags(&s, &plan);
    let apply = step(&out, "Apply this plan: ");
    assert!(
        apply.starts_with(&format!(
            "sweep apply --plan {} --plan-digest ",
            plan.display()
        )),
        "{out}"
    );
    assert!(!apply.contains("SHA"), "{out}");
    assert!(!out.contains("--plan FILE"), "{out}");
    assert!(
        out.contains("        This adds only the tags in this plan and moves nothing. It refuses if anything it scanned has changed."),
        "{out}"
    );
    assert!(!out.contains("This moves only this plan."), "{out}");
    let before = names(&s.root);
    let applied = paste(&s, &s.base, apply);
    assert!(applied.status.success(), "{apply}\n{}", text(&applied));
    assert_eq!(names(&s.root), before, "a file moved");
    for name in ["a.pdf", "b.pdf", "c.pdf"] {
        assert_eq!(tags_of(&s.root.join(name)), ["Receipts"], "{name}");
    }
    assert!(tags_of(&s.root.join("notes.txt")).is_empty());
}

/// A stale tag plan's recovery step makes a tag plan again, not a folder plan.
#[test]
fn a_stale_tag_plan_says_how_to_make_a_tag_plan_again() {
    let s = scene("stale-tags");
    let plan = s.home.join("old.plan");
    let out = export_tags(&s, &plan);
    let apply = step(&out, "Apply this plan: ").to_owned();
    fs::write(s.root.join("d.pdf"), "a file added after the plan").unwrap();
    let refused = paste(&s, &s.base, &apply);
    let said = text(&refused);
    assert_eq!(refused.status.code(), Some(2), "{said}");
    let again = step(&said, "Make a new plan: ");
    assert_eq!(
        again,
        format!(
            "sweep '{}' --scheme tags --map pdf=Receipts --export-plan NEW_PLAN_FILE",
            s.root.display()
        ),
        "{said}"
    );
    assert!(
        said.contains("If you used --depth, --since, --allow-sync or --inspect-content for that plan, add them too, and any --map routes that matched no file."),
        "{said}"
    );
    let fresh = s.home.join("new.plan");
    let remade = paste(
        &s,
        &s.base,
        &format!(
            "{} --since 0",
            again.replace("NEW_PLAN_FILE", fresh.to_str().unwrap())
        ),
    );
    let remade_text = text(&remade);
    assert!(remade.status.success(), "{remade_text}");
    assert!(
        remade_text.contains("Nothing has been tagged or moved."),
        "{remade_text}"
    );
    assert!(
        remade_text.contains("This adds only the tags in this plan"),
        "{remade_text}"
    );
}

/// With no terminal the consent prompt reads no answer, so no contents are
/// read: the footer must not warn that apply skips a check that never ran.
#[test]
fn inspect_content_without_consent_adds_no_content_line() {
    let s = scene("unread");
    let out = text(&sweep(
        &s,
        &s.base,
        &[
            s.root.to_str().unwrap(),
            "--since",
            "0",
            "--inspect-content",
        ],
    ));
    assert!(out.contains("Contents were not read"), "{out}");
    assert!(
        !out.contains("Apply and review do not read file contents"),
        "{out}"
    );
    assert!(
        step(&out, "Save:   ").ends_with("--inspect-content --export-plan ~/sweep.plan"),
        "{out}"
    );
}

/// A plan made with --inspect-content binds that choice only in its digest.
/// The stale step must ask for it, or the new plan drops the content check.
#[test]
fn a_stale_inspected_plan_asks_for_inspect_content_again() {
    let s = scene("stale-inspected");
    let plan = s.home.join("inspected.plan");
    let out = text(&sweep(
        &s,
        &s.base,
        &[
            s.root.to_str().unwrap(),
            "--since",
            "0",
            "--inspect-content",
            "--export-plan",
            plan.to_str().unwrap(),
        ],
    ));
    let apply = step(&out, "Apply this plan: ").to_owned();
    fs::write(s.root.join("d.pdf"), "a file added after the plan").unwrap();
    let refused = paste(&s, &s.base, &apply);
    let said = text(&refused);
    assert_eq!(refused.status.code(), Some(2), "{said}");
    assert!(
        step(&said, "Make a new plan: ").ends_with("--export-plan NEW_PLAN_FILE"),
        "{said}"
    );
    assert!(
        said.contains("If you used --depth, --since, --allow-sync or --inspect-content for that plan, add them too, and any --map routes that matched no file."),
        "{said}"
    );
}
