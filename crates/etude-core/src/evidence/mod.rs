//! Facts about a file that sweep reads from its name and metadata.
//! Evidence describes a file; it never moves or holds one.
//!
//! Design rule: no general clustering. Each detector is individually
//! explainable, high precision, low recall. A user must be able to check the
//! tool's reasoning by eye.
//!
//! Evidence decides nothing. The gate reads it to hold files, and the
//! grouping rules read it to pick folders; both get it from one `observe`
//! call, so each detector runs once per file. Nothing here writes, and
//! nothing here reads a file's contents: every fact comes from the scan's
//! entry alone.

pub mod capture;
pub mod device;
pub mod family;

pub use capture::{Capture, CaptureBasis, CaptureKind};
pub use family::Family;

use crate::scan::{Entry, ScanOutcome};

/// What sweep knows about one scanned entry, before anything is decided.
#[derive(Debug, Clone, Copy)]
pub struct Facts<'a> {
    pub entry: &'a Entry,
    /// The built-in folder its extension files it under, if any.
    pub family: Option<Family>,
    /// Recognised as a screen capture, and how.
    pub capture: Option<Capture>,
    /// Named the way a camera or phone names what it records.
    pub camera: bool,
    /// An installer image or package.
    pub installer: bool,
}

impl<'a> Facts<'a> {
    /// The facts for one entry. Folders get facts too, from their names: the
    /// rules have always looked at every entry the gate let through.
    pub fn of(entry: &'a Entry) -> Facts<'a> {
        Facts {
            entry,
            family: Family::of_extension(&entry.ext),
            capture: capture::recognise_by_known_name(&entry.name, &entry.ext),
            camera: device::is_camera(&entry.name, &entry.ext),
            installer: device::is_installer(&entry.ext),
        }
    }
}

/// The facts for every scanned entry, in scan order.
pub fn observe(scan: &ScanOutcome) -> Vec<Facts<'_>> {
    scan.entries.iter().map(Facts::of).collect()
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
        let shot = entry("Screenshot 2026-07-12 at 9.14.22 AM.png");
        let camera = entry("IMG_4471.HEIC");
        let shot = Facts::of(&shot);
        let camera = Facts::of(&camera);
        assert!(shot.capture.is_some());
        assert!(camera.capture.is_none());
        assert!(camera.camera);
        assert!(!shot.camera);
        assert_eq!(shot.family, Some(Family::Images));
        assert_eq!(camera.family, Some(Family::Images));
    }

    #[test]
    fn an_installer_has_no_family() {
        let dmg = entry("Thing-1.2.dmg");
        let facts = Facts::of(&dmg);
        assert!(facts.installer);
        assert_eq!(facts.family, None);
    }
}
