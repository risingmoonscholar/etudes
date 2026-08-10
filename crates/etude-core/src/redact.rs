//! Path redaction.
//!
//! Error types redact by default so that a stray `{e}` in a call site cannot
//! put a sensitive filename into scrollback, a log, or a crash report.
//! Full paths appear only where the caller formats them deliberately.

use std::path::Path;

/// Render a path with its final component replaced by a shape description.
///
/// `~/Desktop/W2_2024_acme_corp.pdf` becomes `~/Desktop/<name.pdf>`.
pub fn path(p: &Path) -> String {
    let parent = p
        .parent()
        .map(|x| x.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    if parent.is_empty() {
        format!("<name{ext}>")
    } else {
        format!("{parent}/<name{ext}>")
    }
}

/// Render just the shape of a file name, with no directory at all.
pub fn name(n: &str) -> String {
    match n.rsplit_once('.') {
        Some((_, ext)) => format!("<name.{ext}>"),
        None => "<name>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn redacts_the_file_name_but_keeps_the_directory() {
        let p = PathBuf::from("/Users/x/Desktop/W2_2024_acme_corp.pdf");
        let r = path(&p);
        assert!(!r.contains("W2"), "redaction leaked the stem: {r}");
        assert!(r.contains("Desktop"), "redaction lost useful context: {r}");
        assert!(r.contains(".pdf"), "redaction lost the extension: {r}");
    }

    #[test]
    fn redacts_a_bare_name() {
        assert_eq!(name("SSN_card_scan.jpg"), "<name.jpg>");
        assert_eq!(name("id_rsa"), "<name>");
    }
}
