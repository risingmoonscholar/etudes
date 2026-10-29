use std::process::{Command, Stdio};

#[test]
fn missing_input_is_three_and_unknown_option_is_two() {
    let root = std::env::temp_dir().join(format!("etudes-pack-exit-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
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
    std::fs::remove_dir(root).unwrap();
}
