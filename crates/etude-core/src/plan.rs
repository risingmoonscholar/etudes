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
    CameraBurst { days: u32 },
    Installer,
    SharedToken { token: String, count: usize },
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
            Signal::SharedToken { token, count } => {
                format!("{count} filenames contain \"{token}\"")
            }
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
    pub root_is_synced: bool,
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
/// `etude-core` deliberately cannot read file contents itself — the engine has
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
/// `untouched` carries the REASON but never the category for a personal record
/// — an agent gets the count by category, not a list of which files look like
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
                    ("no_clear_group", j::num(self.no_clear_group())),
                    ("no_clear_group_paths", no_group),
                ]),
            ),
            (
                "skipped",
                j::obj(&[
                    ("hidden", j::num(self.skipped_hidden)),
                    ("symlinks", j::num(self.skipped_symlink)),
                ]),
            ),
            ("root_is_synced", j::bool(self.root_is_synced)),
        ])
    }
}

/// Minimum members before a shared token justifies a group. Below this, the
/// grouping is noise and the honest answer is "no clear group".
const MIN_TOKEN_GROUP: usize = 5;
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

    // Pass 1 — the refusal detectors. Run first, remove from all others.
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
        remaining.push(e);
    }

    let mut groups: Vec<Group> = Vec::new();
    let mut claimed: Vec<PathBuf> = Vec::new();

    // Pass 2 — structural detectors, highest precision first.
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

    // Pass 3 — shared tokens. The only detector that names a group from user
    // text, which is exactly why the naming rule permits it.
    let mut by_token: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for e in remaining.iter().filter(|e| !claimed.contains(&e.path)) {
        for t in classify::tokens(&e.name) {
            by_token.entry(t).or_default().push(e.path.clone());
        }
    }
    // Largest groups first, and a file joins only one group.
    let mut candidates: Vec<(String, Vec<PathBuf>)> = by_token
        .into_iter()
        .filter(|(_, v)| v.len() >= MIN_TOKEN_GROUP)
        .collect();
    candidates.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));

    for (token, members) in candidates {
        let fresh: Vec<PathBuf> = members
            .into_iter()
            .filter(|p| !claimed.contains(p))
            .collect();
        if fresh.len() < MIN_TOKEN_GROUP {
            continue;
        }
        claimed.extend(fresh.iter().cloned());
        let count = fresh.len();
        groups.push(Group {
            name: token.clone(),
            signal: Signal::SharedToken { token, count },
            members: fresh,
            accepted: false,
        });
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
        root_is_synced: scan.root_is_synced,
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
            root: PathBuf::from("/fixture"),
            entries,
            skipped_hidden: 0,
            skipped_symlink: 0,
            skipped_system: 0,
            root_is_synced: false,
        }
    }

    #[test]
    fn camera_files_spanning_years_are_not_called_a_burst() {
        // Same filename shape, mtimes years apart. No 3-day window holds
        // enough of them, so no group — and definitely no "taken within
        // 3 days" claim — should be produced.
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
        // Three camera files two days apart, straddling New Year's — a
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
