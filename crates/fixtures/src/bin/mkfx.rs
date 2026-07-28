fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let _ = std::fs::remove_dir_all(&root);
    fixtures::build(&root).unwrap();
    println!("built {}", root.display());
}
