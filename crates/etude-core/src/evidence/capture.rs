//! Screen-capture recognition, and how sure sweep is about it.
//!
//! Today sweep knows one way to recognise a capture: the English name rule it
//! has always used. The types already carry a kind and a basis so that a
//! stronger way to know (the words macOS writes in every language, or an
//! attribute the OS sets) can be added in front of this rule without
//! changing what a caller reads.
//!
//! Nothing here touches the filesystem. Recognition is a pure function of
//! the file name, so it reads no attributes and no contents.

/// What a screen capture holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureKind {
    /// A still screenshot.
    Still,
    /// A screen recording.
    Recording,
}

/// Why sweep believes a file is a screen capture.
///
/// Declared strongest first, and `Ord` follows the declaration, so the
/// weakest basis is the greatest. A group of captures reports the `max` of
/// its files' bases: the output must not claim more than its weakest file
/// supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CaptureBasis {
    /// macOS marked the file as a screen capture with an attribute.
    OsAttribute,
    /// The name starts the way macOS itself writes it, in some language.
    OsName,
    /// The name matches the English rule sweep used before it knew the OS
    /// words. It also covers CleanShot, which is not part of macOS.
    KnownName,
}

/// A file recognised as a screen capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Capture {
    pub kind: CaptureKind,
    pub basis: CaptureBasis,
}

/// The English rule: macOS and common cross-platform screenshot naming. The
/// name starts with `screenshot`, `screen shot` or `cleanshot` in any ASCII
/// case, and the extension is png, jpg, jpeg or heic.
///
/// `ext` is the scan's extension, which is already lower case.
pub fn recognise_by_known_name(file_name: &str, ext: &str) -> Option<Capture> {
    let l = file_name.to_ascii_lowercase();
    let named =
        l.starts_with("screenshot") || l.starts_with("screen shot") || l.starts_with("cleanshot");
    (named && matches!(ext, "png" | "jpg" | "jpeg" | "heic")).then_some(Capture {
        kind: CaptureKind::Still,
        basis: CaptureBasis::KnownName,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_english_rule_is_a_still_known_by_name() {
        let still = Some(Capture {
            kind: CaptureKind::Still,
            basis: CaptureBasis::KnownName,
        });
        for name in [
            "Screenshot 2026-07-12 at 9.14.22 AM.png",
            "Screen Shot 2019-01-01 at 1.00.00 PM.jpg",
            "CleanShot 2026-01-01.heic",
        ] {
            let ext = name.rsplit_once('.').unwrap().1.to_ascii_lowercase();
            assert_eq!(recognise_by_known_name(name, &ext), still, "{name}");
        }
        assert_eq!(recognise_by_known_name("Screenshot notes.txt", "txt"), None);
        assert_eq!(recognise_by_known_name("IMG_4471.HEIC", "heic"), None);
    }

    /// The weakest basis is the greatest, so the `max` over a group is the
    /// basis its weakest file has.
    #[test]
    fn the_weakest_basis_is_the_greatest() {
        assert!(CaptureBasis::OsAttribute < CaptureBasis::OsName);
        assert!(CaptureBasis::OsName < CaptureBasis::KnownName);
    }
}
