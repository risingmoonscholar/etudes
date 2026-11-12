//! Detectors.
//!
//! Design rule: no general clustering. Each detector is individually
//! explainable, high precision, low recall. A user must be able to check the
//! tool's reasoning by eye.
//!
//! Order matters. The safety holds run before any detector here, so a file
//! `sensitive::looks_personal` matches never reaches one.

use crate::scan::Entry;

/// macOS and common cross-platform screenshot naming.
pub fn is_screenshot(e: &Entry) -> bool {
    let l = e.name.to_ascii_lowercase();
    (l.starts_with("screenshot") || l.starts_with("screen shot") || l.starts_with("cleanshot"))
        && matches!(e.ext.as_str(), "png" | "jpg" | "jpeg" | "heic")
}

/// Camera-style stems that indicate an unedited capture.
/// The folder a file's extension files it under, or None to leave it alone.
///
/// Seven families, from the taxonomy spec. The names are the folders users
/// see, so they are Finder words rather than jargon. The list is extensions
/// rather than words because extensions are what a file IS: the rule this
/// replaced grouped files by words they mentioned, and on its first real
/// Downloads folder it made a folder called "apple" out of a receipt, an
/// agreement, a script and an export. Frequency is not category.
///
/// An extension not in this table returns None and the file stays put. The
/// table is the whole mechanism -- nothing here consults the OS, and an app
/// registering its format with macOS changes nothing. Unknown-means-untouched
/// is the decision: filing a format sweep cannot name would mean maintaining
/// a list of every app's private extension forever, which is the stoplist
/// problem as a whitelist.
pub fn type_family(ext: &str) -> Option<&'static str> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "heic" | "dng" | "gif" | "tiff" | "tif" | "webp" | "bmp"
        | "svg" | "raw" => "Images",
        "pdf" | "md" | "docx" | "doc" | "pages" | "txt" | "rtf" | "html" | "htm" | "epub" => {
            "Documents"
        }
        // "Scripts", not "Code", and the name is doing work: it is accurate
        // for interpreted files and it makes the boundary obvious to whoever
        // edits this list next. "Code" would licence putting compiled-language
        // source back, which is exactly what was just removed.
        //
        // Interpreted scripts only. rs, swift and ts were here and are gone:
        // compiled-language source is not a script, and with --depth a source
        // tree could be flattened into Scripts/ before any project guard
        // exists. js stays -- it is at least as often a script as source, and
        // a lone .js in Downloads is far likelier to be a download artifact
        // than a project file. Revisit when the project guard lands.
        "sh" | "py" | "js" | "rb" | "pl" | "zsh" | "bash" => "Scripts",
        // dmg and pkg stay with the existing Installers detector; listing
        // them here too would race it for the same files.
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" => "Archives",
        "mp4" | "mov" | "mp3" | "wav" | "aiff" | "m4a" | "mkv" | "avi" | "flac" | "aac" => "Media",
        // Tabular only, on purpose.
        //
        // json and plist were here and are gone: both are configuration far
        // more often than they are data, and moving an app's config out from
        // under it is the workflow break this family exists to avoid. A
        // config file is not something a person went looking for in their
        // Downloads folder.
        //
        // sqlite is gone too: a database travels with -wal and -shm sidecars,
        // and moving the file without its companions can corrupt it. It comes
        // back when companion-file handling exists, not before.
        "csv" | "xlsx" | "xls" | "parquet" | "numbers" | "tsv" => "Data",
        _ => return None,
    })
}

pub fn is_camera(e: &Entry) -> bool {
    let l = e.name.to_ascii_lowercase();
    let camera_stem = l.starts_with("img_")
        || l.starts_with("dsc")
        || l.starts_with("pxl_")
        || l.starts_with("dji_")
        || l.starts_with("gopro");
    camera_stem
        && matches!(
            e.ext.as_str(),
            "jpg" | "jpeg" | "heic" | "png" | "raw" | "dng" | "mov"
        )
}

pub fn is_installer(e: &Entry) -> bool {
    matches!(
        e.ext.as_str(),
        "dmg" | "pkg" | "msi" | "deb" | "rpm" | "appimage"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(name: &str) -> Entry {
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        Entry {
            path: PathBuf::from(name),
            name: name.to_string(),
            ext,
            size: 1,
            modified: None,
            is_dir: false,
            is_package: false,
        }
    }

    #[test]
    fn screenshots_and_cameras_are_distinguished() {
        assert!(is_screenshot(&entry(
            "Screenshot 2026-07-12 at 9.14.22 AM.png"
        )));
        assert!(!is_screenshot(&entry("IMG_4471.HEIC")));
        assert!(is_camera(&entry("IMG_4471.HEIC")));
        assert!(!is_camera(&entry(
            "Screenshot 2026-07-12 at 9.14.22 AM.png"
        )));
    }
}
