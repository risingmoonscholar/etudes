//! stash clears a folder, tagged items included. sweep holds tagged items by
//! default; stash must not inherit that, or it would leave items behind while
//! reporting the folder clear.
#![cfg(target_os = "macos")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TAG: &str = "com.apple.metadata:_kMDItemUserTags";
const KEY: &str = "0202020202020202020202020202020202020202020202020202020202020202";

struct Scene {
    base: PathBuf,
    root: PathBuf,
    state: PathBuf,
}

impl Drop for Scene {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn scene(label: &str) -> Scene {
    let base = std::env::temp_dir().join(format!(
        "stash-tags-{label}-{}-{}",
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
    Scene {
        root: root.canonicalize().unwrap(),
        base,
        state,
    }
}

fn tag(path: &Path, value: &str) {
    assert!(
        Command::new("xattr")
            .args(["-w", TAG, value])
            .arg(path)
            .status()
            .unwrap()
            .success()
    );
}

fn tag_value(path: &Path) -> Option<String> {
    let out = Command::new("xattr")
        .args(["-p", TAG])
        .arg(path)
        .output()
        .unwrap();
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn stash(scene: &Scene, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stash"))
        .env("ETUDE_STATE_DIR", &scene.state)
        .env("ETUDE_JOURNAL_KEY", KEY)
        .args(args)
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

fn visible(root: &Path) -> Vec<String> {
    fs::read_dir(root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect()
}

#[test]
fn stash_clears_tagged_items_and_reports_the_folder_clear_because_it_is() {
    let s = scene("clear");
    fs::write(s.root.join("plain.txt"), b"plain").unwrap();
    fs::write(s.root.join("tagged.pdf"), b"tagged").unwrap();
    fs::create_dir(s.root.join("tagged-folder")).unwrap();
    fs::write(s.root.join("tagged-folder/inside.txt"), b"inside").unwrap();
    tag(&s.root.join("tagged.pdf"), "red");
    tag(&s.root.join("tagged-folder"), "blue");
    let root = s.root.to_str().unwrap();

    let stashed = stash(&s, &[root]);
    let output = text(&stashed);
    assert!(stashed.status.success(), "{output}");
    assert!(output.contains("Stashed 3 items."), "{output}");
    assert!(output.contains("The selected folder is clear."), "{output}");
    assert!(!output.contains("NOT clear"), "{output}");
    assert_eq!(
        visible(&s.root),
        Vec::<String>::new(),
        "nothing left behind"
    );

    // Everything is in the holding folder, tags intact.
    let holding = fs::read_dir(&s.root)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(".stash-"))
        })
        .expect("holding folder");
    assert_eq!(
        tag_value(&holding.join("tagged.pdf")).as_deref(),
        Some("red")
    );
    assert_eq!(
        tag_value(&holding.join("tagged-folder")).as_deref(),
        Some("blue")
    );

    let popped = stash(&s, &["pop", root]);
    assert!(popped.status.success(), "{}", text(&popped));
    assert_eq!(
        tag_value(&s.root.join("tagged.pdf")).as_deref(),
        Some("red")
    );
    assert!(s.root.join("tagged-folder/inside.txt").exists());
}

#[test]
fn stash_json_says_clear_and_counts_nothing_held() {
    let s = scene("json");
    fs::write(s.root.join("tagged.txt"), b"x").unwrap();
    tag(&s.root.join("tagged.txt"), "red");
    let stashed = stash(&s, &[s.root.to_str().unwrap(), "--json"]);
    let json = String::from_utf8_lossy(&stashed.stdout).into_owned();
    assert!(stashed.status.success(), "{json}");
    assert!(json.contains("\"clear\":true"), "{json}");
    assert!(json.contains("\"held_cross_volume\":0"), "{json}");
    assert!(json.contains("\"held_tagged\":0"), "{json}");
    assert!(!json.contains("\"red\""), "{json}");
    assert_eq!(visible(&s.root), Vec::<String>::new());
}

#[test]
fn stash_select_takes_a_tagged_object_the_caller_named() {
    let s = scene("select");
    let file = s.root.join("tagged.txt");
    fs::write(&file, b"x").unwrap();
    tag(&file, "green");
    let selected = stash(&s, &["select", file.to_str().unwrap()]);
    assert!(selected.status.success(), "{}", text(&selected));
    assert!(!file.exists(), "the selected tagged file was stashed");
}
