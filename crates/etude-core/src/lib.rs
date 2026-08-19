//! etude-core: the classification engine.
//!
//! Zero dependencies. Nothing in this crate can open a socket, because there is
//! no third-party code here at all.
//!
//! Scanning and classification read **filesystem metadata only** in v0.1:
//! names, sizes, timestamps, and file type. They never open files for reading.
//! The journal's edge-hash step does: it reads file bytes to detect changes
//! before undo.

pub mod apply;
pub mod classify;
pub mod journal;
pub mod json;
pub mod plan;
pub mod redact;
pub mod scan;

pub use apply::{ApplyReport, UndoReport};
pub use journal::Journal;
pub use plan::{Group, Plan, Signal};
pub use scan::{Entry, ScanConfig, ScanError, ScanOutcome};

/// Why sweep declined to act on a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Untouched {
    /// Matched a personal-records pattern. Never grouped, never moved.
    LooksPersonal(Category),
    /// No detector claimed it with enough confidence.
    NoClearGroup,
    /// Changed within the grace window. Too recent to judge: a file someone
    /// touched an hour ago is one they are using, and a file that arrived an
    /// hour ago has not been dealt with yet. Either way, moving it now is
    /// moving something out from under a person mid-task.
    TooRecent,
    /// A download still in flight. Moving one produces a partial file at a
    /// destination the downloader is not writing to, and it never completes.
    InFlight,
}

/// Categories of personal record sweep recognises well enough to refuse.
///
/// These names are used in **counts only**. Never to name a destination
/// directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    Tax,
    Identity,
    Medical,
    Financial,
    Credential,
    Legal,
}

impl Category {
    pub fn describe(self) -> &'static str {
        match self {
            Category::Tax => "tax documents",
            Category::Identity => "identity documents",
            Category::Medical => "medical records",
            Category::Financial => "financial records",
            Category::Credential => "credentials or keys",
            Category::Legal => "legal documents",
        }
    }
}
