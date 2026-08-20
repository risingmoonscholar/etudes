//! Plan construction. Nothing here writes to the filesystem.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::classify;
use crate::scan::{Entry, ScanOutcome};
use crate::{Category, Untouched};

/// Why a group exists, in words the user can check by eye.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    Screenshot,
    CameraBurst {
        days: u32,
    },
    Installer,
    /// Files grouped by what they are: a type family from the extension.
    /// Carries the extensions actually present so the explanation names them
    /// rather than asserting a category the user has to take on trust.
    TypeFamily {
        exts: Vec<String>,
    },
    /// Not a detector. A caller-supplied set already decided elsewhere --
    /// stash's holding folder is one group by definition. Was called
    /// SharedToken, back when a rule grouped files by words they contained;
    /// that rule is gone and the name described nothing.
    Collected {
        count: usize,
    },
}

impl Signal {
    /// The explanation printed beside the group. Never invents a claim.
    pub fn describe(&self) -> String {
        match self {
            Signal::Screenshot => "named \"Screenshot ...\"".to_string(),
            Signal::CameraBurst { days } => {
                format!("camera names, taken within {days} days")
            }
            Signal::Installer => ".dmg and .pkg".to_string(),
            Signal::TypeFamily { exts } => {
                let mut e: Vec<&str> = exts.iter().map(String::as_str).collect();
                e.sort_unstable();
                e.dedup();
                format!(".{}", e.join(", ."))
            }
            Signal::Collected { count } => format!("{count} items"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Group {
    /// Destination directory name. Always a string the filenames already
    /// contained, or a structural fact. Never a coined label.
    pub name: String,
    pub signal: Signal,
    pub members: Vec<PathBuf>,
    /// Set by `sweep review`. Only accepted groups are applied.
    pub accepted: bool,
}

#[derive(Debug)]
pub struct Plan {
    pub root: PathBuf,
    pub groups: Vec<Group>,
    /// Files sweep declined to act on, with the reason.
    pub untouched: Vec<(PathBuf, Untouched)>,
    pub scanned: usize,
    pub skipped_hidden: usize,
    pub skipped_symlink: usize,
    /// Entries refused by policy. See `ScanOutcome::skipped_system`.
    pub skipped_system: usize,
    /// Directories not entered because they hold a project marker.
    pub skipped_project: usize,
    pub skipped_in_flight: usize,
    /// Directories that could not be read at all. See
    /// `ScanOutcome::skipped_unreadable`. A nonzero value here means
    /// `scanned` is a floor, not a total: some part of the tree was
    /// completely invisible to this scan.
    pub skipped_unreadable: usize,
    pub root_is_synced: bool,
    /// The `--allow-sync` this plan's scan was actually run with. Copied
    /// straight from `ScanOutcome::allow_sync`. NOT derived from
    /// `root_is_synced`, on purpose: a caller can pass `--allow-sync` on a
    /// root that isn't itself synced (`root_is_synced` false) and later
    /// have a destination collide with a sync marker anyway (e.g. a `sweep
    /// review` rename to "Dropbox"). Deriving this from `root_is_synced`
    /// would silently drop that consent. This is what `apply` consults
    /// instead of re-deciding: the decision was already made when the plan
    /// was built, and apply never received the original flag on its own.
    pub allow_sync: bool,
}

impl Plan {
    /// Counts per sensitive category, for the summary line. Used instead of
    /// naming the files, and never used to name a destination directory.
    pub fn sensitive_counts(&self) -> BTreeMap<Category, usize> {
        let mut m = BTreeMap::new();
        for (_, u) in &self.untouched {
            if let Untouched::LooksPersonal(c) = u {
                *m.entry(*c).or_insert(0) += 1;
            }
        }
        m
    }

    /// Held back by the grace window.
    pub fn too_recent(&self) -> usize {
        self.untouched
            .iter()
            .filter(|(_, u)| *u == Untouched::TooRecent)
            .count()
    }

    /// Downloads still in flight.
    pub fn in_flight(&self) -> usize {
        self.untouched
            .iter()
            .filter(|(_, u)| *u == Untouched::InFlight)
            .count()
    }

    pub fn no_clear_group(&self) -> usize {
        self.untouched
            .iter()
            .filter(|(_, u)| *u == Untouched::NoClearGroup)
            .count()
    }

    pub fn moves(&self) -> usize {
        self.groups
            .iter()
            .filter(|g| g.accepted)
            .map(|g| g.members.len())
            .sum()
    }
}

/// Optional content inspection, injected by the caller.
///
/// `etude-core` deliberately cannot read file contents itself. The engine has
/// zero dependencies and never opens a file. When the user passes
/// `--inspect-content` the CLI supplies an implementation.
///
/// The return type is the whole safety story: `Option<Category>` and nothing
/// else. There is no way for an inspector to suggest a group, a label, or a
/// destination, so content can only ever widen refusal.
pub trait Inspector {
    fn inspect(&mut self, path: &std::path::Path, ext: &str) -> Option<Category>;
}

/// The machine-readable form of a plan.
///
/// The agent contract: this is what `--json` emits, and it is the same data the
/// human rendering is drawn from. Both must agree, because a tool that tells a
/// person one thing and an agent another is the worst kind of interface.
///
/// `untouched` carries the REASON but never the category for a personal record.
/// An agent gets the count by category, not a list of which files look like
/// tax documents. That would hand it exactly the index the naming rule exists
/// to prevent.
impl Plan {
    pub fn to_json(&self) -> String {
        use crate::json as j;

        let groups = j::arr(self.groups.iter().map(|g| {
            j::obj(&[
                ("name", j::str(&g.name)),
                ("signal", j::str(&g.signal.describe())),
                ("accepted", j::bool(g.accepted)),
                ("count", j::num(g.members.len())),
                ("members", j::arr(g.members.iter().map(|m| j::path(m)))),
            ])
        }));

        let personal: usize = self.sensitive_counts().values().sum();
        let by_category = j::arr(
            self.sensitive_counts()
                .iter()
                .map(|(c, n)| j::obj(&[("kind", j::str(c.describe())), ("count", j::num(n))])),
        );

        // Only the paths sweep declined for lack of a group. Personal-looking
        // files are counted, never listed.
        let no_group = j::arr(
            self.untouched
                .iter()
                .filter(|(_, u)| *u == Untouched::NoClearGroup)
                .map(|(p, _)| j::path(p)),
        );

        j::obj(&[
            ("root", j::path(&self.root)),
            ("scanned", j::num(self.scanned)),
            ("groups", groups),
            (
                "left_alone",
                j::obj(&[
                    ("looks_personal", j::num(personal)),
                    ("by_category", by_category),
                    ("too_recent", j::num(self.too_recent())),
                    ("projects_skipped", j::num(self.skipped_project)),
                    ("downloads_skipped", j::num(self.skipped_in_flight)),
                    ("in_flight", j::num(self.in_flight())),
                    ("no_clear_group", j::num(self.no_clear_group())),
                    ("no_clear_group_paths", no_group),
                ]),
            ),
            (
                "skipped",
                j::obj(&[
                    ("hidden", j::num(self.skipped_hidden)),
                    ("symlinks", j::num(self.skipped_symlink)),
                    // A deliberate refusal: sweep could have entered these
                    // and chose not to (credential/noise names, or a system
                    // location like /Library).
                    ("refused_system_location", j::num(self.skipped_system)),
                    // NOT a choice: read_dir() itself failed (permission
                    // denied, path too long, ...). A nonzero count here
                    // means `scanned` does not describe everything that was
                    // there. An agent must not treat it as a total.
                    ("unreadable", j::num(self.skipped_unreadable)),
                ]),
            ),
            ("root_is_synced", j::bool(self.root_is_synced)),
        ])
    }
}

/// Minimum members before a shared token justifies a group. Below this, the
/// grouping is noise and the honest answer is "no clear group".
const MIN_STRUCTURAL_GROUP: usize = 3;
/// Window within which camera files count as one burst.
const BURST_DAYS: u32 = 3;

/// Build a plan from metadata alone. Pure: reads no file contents.
pub fn build(scan: &ScanOutcome) -> Plan {
    build_with(scan, None)
}

/// Build a plan, optionally inspecting contents to refuse more files.
///
/// The inspector runs in pass 1 only, alongside the filename check, and its
/// result can do exactly one thing: move a file into the untouched set.
pub fn build_with(scan: &ScanOutcome, mut inspector: Option<&mut dyn Inspector>) -> Plan {
    let mut untouched: Vec<(PathBuf, Untouched)> = Vec::new();
    let mut remaining: Vec<&Entry> = Vec::new();

    // Pass 1: the refusal detectors. Run first, remove from all others.
    for e in &scan.entries {
        // Filename first: it is free, and a file already refused by name is
        // never opened. Reading it could only confirm what we already decided.
        if let Some(cat) = classify::sensitive(e) {
            untouched.push((e.path.clone(), Untouched::LooksPersonal(cat)));
            continue;
        }
        if let Some(insp) = inspector.as_deref_mut()
            && let Some(cat) = insp.inspect(&e.path, &e.ext)
        {
            untouched.push((e.path.clone(), Untouched::LooksPersonal(cat)));
            continue;
        }
        // A download still running. Moving one leaves a partial file at a
        // destination the downloader is not writing to, and it never
        // finishes. Checked before the grace window because it is true
        // regardless of age -- a stalled download from last week is still in
        // flight.
        let lower = e.name.to_ascii_lowercase();
        if crate::scan::IN_FLIGHT_SUFFIXES
            .iter()
            .any(|s| lower.ends_with(s))
        {
            untouched.push((e.path.clone(), Untouched::InFlight));
            continue;
        }
        // Too recent to judge. See ScanConfig::grace for why this is mtime
        // and never atime.
        // `--since 0` means no window, and it has to mean that for every
        // file. elapsed() returns Err for a timestamp in the future, and
        // unwrap_or(true) turned that error into "too recent" even when the
        // window was zero -- so three PDFs dated 2030 were held back by a
        // scan that had been told to hold nothing back. Future mtimes are
        // ordinary: clock skew, restored backups, unpacked archives, network
        // volumes.
        //
        // With a real window a future mtime still counts as too recent. That
        // is the conservative reading of a file whose clock disagrees with
        // this one, and it is now a decision rather than the fallback of an
        // unwrap.
        // A file whose mtime could not be read, with a window in force. The
        // window's question is "was this changed recently", and the honest
        // answer here is that sweep does not know -- so it holds the file
        // back rather than treating unknown as old. Bypassing the check meant
        // an unreadable timestamp was silently the most permissive case.
        if let Some(window) = scan.grace
            && !window.is_zero()
            && e.modified.is_none()
        {
            untouched.push((e.path.clone(), Untouched::TooRecent));
            continue;
        }
        if let Some(window) = scan.grace
            && !window.is_zero()
            && let Some(modified) = e.modified
            && modified
                .elapsed()
                .map(|since| since < window)
                .unwrap_or(true)
        {
            untouched.push((e.path.clone(), Untouched::TooRecent));
            continue;
        }
        remaining.push(e);
    }

    let mut groups: Vec<Group> = Vec::new();
    let mut claimed: Vec<PathBuf> = Vec::new();

    // Pass 2: structural detectors, highest precision first.
    let shots: Vec<&Entry> = remaining
        .iter()
        .copied()
        .filter(|e| classify::is_screenshot(e))
        .collect();
    if shots.len() >= MIN_STRUCTURAL_GROUP {
        claimed.extend(shots.iter().map(|e| e.path.clone()));
        groups.push(Group {
            name: "Screenshots".to_string(),
            signal: Signal::Screenshot,
            members: shots.iter().map(|e| e.path.clone()).collect(),
            accepted: false,
        });
    }

    let mut camera_candidates: Vec<&Entry> = remaining
        .iter()
        .copied()
        .filter(|e| classify::is_camera(e) && !claimed.contains(&e.path) && e.modified.is_some())
        .collect();
    camera_candidates.sort_by_key(|e| e.modified);
    let burst_window = Duration::from_secs(u64::from(BURST_DAYS) * 86_400);
    let mut window_start = 0;
    let mut best_start = 0;
    let mut best_len = 0;
    for window_end in 0..camera_candidates.len() {
        while camera_candidates[window_end]
            .modified
            .unwrap()
            .duration_since(camera_candidates[window_start].modified.unwrap())
            .unwrap()
            > burst_window
        {
            window_start += 1;
        }
        let window_len = window_end - window_start + 1;
        if window_len > best_len {
            best_start = window_start;
            best_len = window_len;
        }
    }
    let cams: Vec<&Entry> = camera_candidates[best_start..best_start + best_len].to_vec();
    if cams.len() >= MIN_STRUCTURAL_GROUP {
        claimed.extend(cams.iter().map(|e| e.path.clone()));
        let name = match date_range(&cams) {
            Some(r) => format!("Photos, {r}"),
            None => "Photos".to_string(),
        };
        groups.push(Group {
            name,
            signal: Signal::CameraBurst { days: BURST_DAYS },
            members: cams.iter().map(|e| e.path.clone()).collect(),
            accepted: false,
        });
    }

    let inst: Vec<&Entry> = remaining
        .iter()
        .copied()
        .filter(|e| classify::is_installer(e) && !claimed.contains(&e.path))
        .collect();
    if inst.len() >= MIN_STRUCTURAL_GROUP {
        claimed.extend(inst.iter().map(|e| e.path.clone()));
        groups.push(Group {
            name: "Installers".to_string(),
            signal: Signal::Installer,
            members: inst.iter().map(|e| e.path.clone()).collect(),
            accepted: false,
        });
    }

    // There is deliberately no pass grouping by shared words. One existed:
    // any token in five or more filenames became a folder named after that
    // token, guarded by a hand-written stoplist. On the first real Downloads
    // folder it met, it made a folder called "apple" out of a receipt, an
    // agreement, a script and an export that shared a word, and the stoplist
    // could never have saved it -- the set of words that are not categories
    // is the whole vocabulary minus a few dozen. Frequency is not category.
    //
    // Its lifetime record on data this repo did not author: zero true
    // positives, one false positive. Both showcase groups in the fixture were
    // planted to demonstrate it. The one thing it was right about -- a
    // project cluster spanning extensions -- is caught by a better,
    // observable signal: a project file marks its folder as one unit.
    //
    // Two independent reviews converged here: one flagged the stoplist as
    // limited, the other watched the rule produce "apple". Do not
    // reintroduce a token pass; extend the structural detectors instead.

    // Pass 3: type families. A file's extension says what it IS, which is the
    // question a folder name should answer. Extensions the table does not
    // know are left alone rather than swept into an "Other" drawer. The table
    // is the whole mechanism -- the OS is not consulted -- and unknown means
    // untouched, because filing formats sweep cannot name would mean
    // maintaining a list of every app's private extension forever.
    {
        let mut by_family: BTreeMap<&'static str, Vec<&Entry>> = BTreeMap::new();
        for e in remaining.iter().filter(|e| !claimed.contains(&e.path)) {
            if let Some(fam) = classify::type_family(&e.ext) {
                by_family.entry(fam).or_default().push(e);
            }
        }
        for (family, members) in by_family {
            if members.len() < MIN_STRUCTURAL_GROUP {
                continue;
            }
            claimed.extend(members.iter().map(|e| e.path.clone()));
            groups.push(Group {
                name: family.to_string(),
                signal: Signal::TypeFamily {
                    exts: members.iter().map(|e| e.ext.to_lowercase()).collect(),
                },
                members: members.iter().map(|e| e.path.clone()).collect(),
                accepted: false,
            });
        }
    }

    // Everything still unclaimed is honestly reported as ungrouped.
    for e in remaining.iter().filter(|e| !claimed.contains(&e.path)) {
        untouched.push((e.path.clone(), Untouched::NoClearGroup));
    }

    untouched.sort_by(|a, b| a.0.cmp(&b.0));

    Plan {
        root: scan.root.clone(),
        groups,
        untouched,
        scanned: scan.entries.len(),
        skipped_hidden: scan.skipped_hidden,
        skipped_symlink: scan.skipped_symlink,
        skipped_system: scan.skipped_system,
        skipped_project: scan.skipped_project,
        skipped_in_flight: scan.skipped_in_flight,
        skipped_unreadable: scan.skipped_unreadable,
        root_is_synced: scan.root_is_synced,
        allow_sync: scan.allow_sync,
    }
}

/// Human date range for a set of entries, from mtime only.
fn date_range(entries: &[&Entry]) -> Option<String> {
    let mut times: Vec<SystemTime> = entries.iter().filter_map(|e| e.modified).collect();
    if times.is_empty() {
        return None;
    }
    times.sort();
    let lo = times
        .first()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())?
        .as_secs();
    let hi = times
        .last()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())?
        .as_secs();
    let ((lo_year, a), (hi_year, b)) = (civil_date(lo as i64), civil_date(hi as i64));
    Some(if lo_year == hi_year && a == b {
        a
    } else if lo_year == hi_year {
        format!("{a}–{b}, {lo_year}")
    } else {
        format!("{a}, {lo_year}–{b}, {hi_year}")
    })
}

/// Days-from-epoch to `Mon D`, without pulling in a date crate.
/// Howard Hinnant's civil_from_days.
fn civil_date(secs: i64) -> (i64, String) {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, format!("{} {}", MONTHS[(m - 1) as usize], d))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam_entry(name: &str, modified_secs: u64) -> Entry {
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        Entry {
            path: PathBuf::from(name),
            name: name.to_string(),
            ext,
            size: 1,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(modified_secs)),
            is_dir: false,
            is_package: false,
        }
    }

    fn scan_outcome(entries: Vec<Entry>) -> ScanOutcome {
        ScanOutcome {
            grace: None,
            root: PathBuf::from("/fixture"),
            entries,
            skipped_hidden: 0,
            skipped_symlink: 0,
            skipped_system: 0,
            skipped_project: 0,
            skipped_in_flight: 0,
            skipped_unreadable: 0,
            root_is_synced: false,
            allow_sync: false,
        }
    }

    /// A file whose mtime cannot be read is held back, not swept.
    ///
    /// The grace window asks "was this changed recently". When the timestamp
    /// is unreadable the honest answer is that sweep does not know, and the
    /// old code answered "no" by skipping the check entirely -- so an
    /// unreadable timestamp was quietly the most permissive case there is.
    #[test]
    fn a_file_whose_age_cannot_be_read_is_held_back_by_a_live_window() {
        let mut entries: Vec<Entry> = (0..3)
            .map(|i| cam_entry(&format!("report_{i}.pdf"), 0))
            .collect();
        for e in &mut entries {
            e.modified = None;
        }
        let mut scan = scan_outcome(entries);
        scan.grace = Some(Duration::from_secs(24 * 60 * 60));

        let p = build(&scan);
        assert_eq!(
            p.too_recent(),
            3,
            "files with an unreadable mtime were swept by a live grace window"
        );
        assert!(
            p.groups.is_empty(),
            "an unreadable timestamp must not be the most permissive case"
        );

        // With the window off, they are eligible again -- zero means zero.
        scan.grace = Some(Duration::ZERO);
        let p = build(&scan);
        assert_eq!(
            p.too_recent(),
            0,
            "--since 0 must hold nothing back, unreadable mtime included"
        );
    }

    #[test]
    fn camera_files_spanning_years_are_not_called_a_burst() {
        // Same filename shape, mtimes years apart. No 3-day window holds
        // enough of them, so no group should be produced. Definitely no
        // "taken within 3 days" claim should be made either.
        let entries = vec![
            cam_entry("IMG_0001.jpg", 1_577_836_800), // 2020-01-01
            cam_entry("IMG_0002.jpg", 1_655_251_200), // 2022-06-15
            cam_entry("IMG_0003.jpg", 1_710_028_800), // 2024-03-10
            cam_entry("IMG_0004.jpg", 1_767_225_600), // 2026-01-01
        ];
        let out = scan_outcome(entries);
        let p = build(&out);
        assert!(
            !p.groups
                .iter()
                .any(|g| matches!(g.signal, Signal::CameraBurst { .. })),
            "camera files years apart were grouped as a burst: {:?}",
            p.groups
        );
    }

    #[test]
    fn real_burst_across_a_year_boundary_names_both_years() {
        // Three camera files two days apart, straddling New Year's. A
        // genuine burst that also exercises the multi-year label.
        let entries = vec![
            cam_entry("IMG_0001.jpg", 1_767_139_200), // 2025-12-31
            cam_entry("IMG_0002.jpg", 1_767_225_600), // 2026-01-01
            cam_entry("IMG_0003.jpg", 1_767_312_000), // 2026-01-02
        ];
        let out = scan_outcome(entries);
        let p = build(&out);
        let burst = p
            .groups
            .iter()
            .find(|g| matches!(g.signal, Signal::CameraBurst { .. }))
            .expect("three camera files within 3 days should form a burst group");
        assert_eq!(burst.members.len(), 3);
        assert_eq!(burst.name, "Photos, Dec 31, 2025–Jan 2, 2026");
    }

    #[test]
    fn date_range_does_not_collapse_a_multi_year_span_to_one_date() {
        let a = cam_entry("a.jpg", 1_577_836_800); // 2020-01-01
        let b = cam_entry("b.jpg", 1_767_225_600); // 2026-01-01
        let range = date_range(&[&a, &b]).expect("both entries have modified times");
        assert_ne!(
            range, "Jan 1",
            "multi-year range collapsed to a single date"
        );
        assert_eq!(range, "Jan 1, 2020–Jan 1, 2026");
    }
}
