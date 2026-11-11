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
/// printed, moves the files.
fn save_then_apply(s: &Scene, cwd: &Path, path: &str) {
    let out = text(&sweep(s, cwd, &[path, "--since", "0"]));
    let save = step(&out, "Save:   ");
    assert!(save.ends_with("--export-plan ~/sweep.plan"), "{out}");
    let saved = paste(s, cwd, save);
    let saved_text = text(&saved);
    assert!(saved.status.success(), "{save}\n{saved_text}");
    assert!(s.home.join("sweep.plan").is_file(), "{saved_text}");
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
    save_then_apply(&s, &s.root, ".");
}

#[test]
fn the_save_line_pastes_from_outside_the_folder() {
    let s = scene("outside");
    save_then_apply(&s, &s.base, s.root.to_str().unwrap());
}

/// After an export, a tag plan prints the exact apply with the real digest,
/// not the generic `--plan FILE --plan-digest SHA`.
#[test]
fn an_exported_tag_plan_prints_the_exact_apply() {
    let s = scene("tags");
    let plan = s.home.join("tags.plan");
    let out = text(&sweep(
        &s,
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
    ));
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
}
