//! The type families: which built-in folder a file's extension files it under.

/// A built-in folder that files are grouped into by extension.
///
/// Six of the seven families from the taxonomy spec. The seventh, Installers,
/// has its own detector (`device::is_installer`), so its extensions are not
/// listed here: listing them in both would race the two for the same files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    Images,
    Documents,
    Scripts,
    Archives,
    Media,
    Data,
}

impl Family {
    /// Every family, in the order the taxonomy lists them.
    pub const ALL: [Family; 6] = [
        Family::Images,
        Family::Documents,
        Family::Scripts,
        Family::Archives,
        Family::Media,
        Family::Data,
    ];

    /// The folder a file's extension files it under, or None to leave it alone.
    ///
    /// The names are the folders users see, so they are Finder words rather
    /// than jargon. The list is extensions rather than words because
    /// extensions are what a file IS: the rule this replaced grouped files by
    /// words they mentioned, and on its first real Downloads folder it made a
    /// folder called "apple" out of a receipt, an agreement, a script and an
    /// export. Frequency is not category.
    ///
    /// An extension not in this table returns None and the file stays put.
    /// The table is the whole mechanism -- nothing here consults the OS, and
    /// an app registering its format with macOS changes nothing.
    /// Unknown-means-untouched is the decision: filing a format sweep cannot
    /// name would mean maintaining a list of every app's private extension
    /// forever, which is the stoplist problem as a whitelist.
    pub fn of_extension(ext: &str) -> Option<Family> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "png" | "jpg" | "jpeg" | "heic" | "dng" | "gif" | "tiff" | "tif" | "webp" | "bmp"
            | "svg" | "raw" => Family::Images,
            "pdf" | "md" | "docx" | "doc" | "pages" | "txt" | "rtf" | "html" | "htm" | "epub" => {
                Family::Documents
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
            "sh" | "py" | "js" | "rb" | "pl" | "zsh" | "bash" => Family::Scripts,
            // dmg and pkg stay with the existing Installers detector; listing
            // them here too would race it for the same files.
            "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" => Family::Archives,
            "mp4" | "mov" | "mp3" | "wav" | "aiff" | "m4a" | "mkv" | "avi" | "flac" | "aac" => {
                Family::Media
            }
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
            "csv" | "xlsx" | "xls" | "parquet" | "numbers" | "tsv" => Family::Data,
            _ => return None,
        })
    }

    /// The folder this family's files are grouped into. The only place the
    /// six folder names are written: the type-family groups, the count of
    /// files below the three-file floor and the names `--map` may not use
    /// all ask this.
    pub fn folder_name(self) -> &'static str {
        match self {
            Family::Images => "Images",
            Family::Documents => "Documents",
            Family::Scripts => "Scripts",
            Family::Archives => "Archives",
            Family::Media => "Media",
            Family::Data => "Data",
        }
    }

    /// Whether a loose project document could point at a file of this family:
    /// a .flp's samples are Media, a .blend's textures are Images. A file that
    /// can be referenced is held beside a project document, so moving it
    /// cannot break the project. Documents and archives are not referenced
    /// this way, so the PDFs and zips beside a project still sort.
    ///
    /// This is the one list. The gate's near-document hold and the scan's
    /// project-folder hold both ask it.
    pub fn can_be_referenced(self) -> bool {
        match self {
            Family::Media | Family::Images | Family::Scripts | Family::Data => true,
            Family::Documents | Family::Archives => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The folder names are part of the output: a plan names them, and a
    /// folder of that name already here changes what a plan does.
    #[test]
    fn each_family_has_its_folder_name() {
        let names: Vec<&str> = Family::ALL.iter().map(|f| f.folder_name()).collect();
        assert_eq!(
            names,
            [
                "Images",
                "Documents",
                "Scripts",
                "Archives",
                "Media",
                "Data"
            ]
        );
    }

    /// The families a project document can reference are exactly the four the
    /// near-document hold has always held.
    #[test]
    fn media_images_scripts_and_data_can_be_referenced() {
        let referenced: Vec<&str> = Family::ALL
            .iter()
            .filter(|f| f.can_be_referenced())
            .map(|f| f.folder_name())
            .collect();
        assert_eq!(referenced, ["Images", "Scripts", "Media", "Data"]);
    }

    #[test]
    fn extension_case_does_not_matter_and_unknown_is_none() {
        assert_eq!(Family::of_extension("PNG"), Some(Family::Images));
        assert_eq!(Family::of_extension("csv"), Some(Family::Data));
        assert_eq!(Family::of_extension("dmg"), None);
        assert_eq!(Family::of_extension("bpy"), None);
        assert_eq!(Family::of_extension(""), None);
    }
}
