use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(arg) = std::env::args().nth(1) else {
        eprintln!("usage: mkfx <dir>");
        return ExitCode::from(2);
    };
    let root = std::path::PathBuf::from(arg);
    let _ = std::fs::remove_dir_all(&root);
    if let Err(e) = fixtures::build(&root) {
        eprintln!("mkfx: {}: {e}", root.display());
        return ExitCode::from(3);
    }
    println!("built {}", root.display());
    ExitCode::SUCCESS
}
