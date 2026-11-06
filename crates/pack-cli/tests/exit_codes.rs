use std::process::{Command, Stdio};

/// Removes the test folder on drop, so a failed assertion leaves nothing.
struct Root(std::path::PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn missing_input_is_three_and_unknown_option_is_two() {
    let root = std::env::temp_dir().join(format!("etudes-pack-exit-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let _cleanup = Root(root.clone());
    for (args, code, reason) in [
        (
            vec![
                root.join("absent").to_string_lossy().into_owned(),
                "--for".into(),
                "other".into(),
                "--json".into(),
            ],
            3,
            "missing_input",
        ),
        (
            vec!["--unknown".into(), "--json".into()],
            2,
            "unknown_option",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_pack"))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(code));
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains(&format!(r#""code":"{reason}""#))
        );
    }
}
