//! The mode: where the files the gate let through go.
//!
//! A plan is built in three steps. The evidence describes each file, the gate
//! holds every file that is not safe to touch, and the mode decides what
//! happens to the rest. Each way to organize a folder is one `Mode`.
//!
//! A folder mode is an ordered list of rules. Each rule looks at the
//! candidates no earlier rule claimed and proposes groups; it never moves,
//! holds or claims a file itself. `settle` then decides each proposal: the
//! three-file floor for a folder sweep would have to make, and the files
//! already inside a folder that is here. The first rule to claim a file wins,
//! which is what makes the order mean something.
//!
//! The tag mode moves nothing, so it has no folder rules: it has its own claim
//! step over the gate's candidates.
//!
//! A new way to organize a folder is a new `Mode` with its own rule list. It
//! gets the gate, the claim set, `settle` and the plan with no changes.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::Untouched;
use crate::evidence::Facts;
use crate::plan::{Group, MapSpec, Signal, TagSpec};
use crate::scan::{Entry, ScanOutcome};

/// One way to organize a folder: the `--scheme` the person chose, with what
/// it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// `--scheme folders`: sweep's own rules, then any `--map` routes, which
    /// may take only what those rules leave.
    Folders { maps: Vec<MapSpec> },
    /// `--scheme existing`: the person's saved routes, then sweep's own
    /// rules. `--map` cannot be combined with it.
    Existing { routes: Vec<MapSpec> },
    /// `--scheme tags`: files get a Finder tag and stay where they are.
    Tags { specs: Vec<TagSpec> },
}

impl Default for Mode {
    /// `sweep PATH` with no scheme and no `--map`.
    fn default() -> Self {
        Mode::Folders { maps: Vec::new() }
    }
}

/// What a mode decided about the gate's candidates.
pub(crate) struct Claimed {
    pub(crate) groups: Vec<Group>,
    /// Candidates no group moves, with the reason, in the order decided.
    pub(crate) untouched: Vec<(PathBuf, Untouched)>,
}

impl Mode {
    /// Decide every candidate the gate let through: each one ends in a group
    /// or in the left-alone list, never both.
    pub(crate) fn claim(&self, scan: &ScanOutcome, candidates: &[&Facts]) -> Claimed {
        match self {
            Mode::Tags { specs } => tag(candidates, specs),
            Mode::Folders { .. } | Mode::Existing { .. } => self.claim_by_rules(scan, candidates),
        }
    }

    /// A tag plan adds a tag and moves nothing, so no group has a folder.
    pub(crate) fn adds_tags(&self) -> bool {
        matches!(self, Mode::Tags { .. })
    }

    /// The folder rules, in the order they claim files.
    fn rules(&self) -> Vec<Rule<'_>> {
        const BUILT_IN: [Rule<'static>; 4] = [
            Rule::ScreenCaptures,
            Rule::CameraBurst,
            Rule::Installers,
            Rule::TypeFamilies,
        ];
        match self {
            Mode::Folders { maps } => {
                let mut rules = BUILT_IN.to_vec();
                rules.push(Rule::OneShotMaps(maps));
                rules
            }
            Mode::Existing { routes } => {
                let mut rules = vec![Rule::SavedRoutes(routes)];
                rules.extend(BUILT_IN);
                rules
            }
            Mode::Tags { .. } => Vec::new(),
        }
    }

    /// The claim loop: each rule proposes groups from what is still
    /// unclaimed, and `settle` decides each proposal. Whatever no rule
    /// claimed is reported as having no group.
    fn claim_by_rules(&self, scan: &ScanOutcome, candidates: &[&Facts]) -> Claimed {
        let mut claimed = ClaimSet::default();
        let mut groups = Vec::new();
        let mut untouched = Vec::new();
        for rule in self.rules() {
            for mut proposal in rule.propose(candidates, &claimed) {
                // A rule's proposals see the claims made before the rule ran.
                // The built-in rules never propose one file twice, but a
                // file settled into an earlier proposal of the same rule is
                // still that proposal's: the first claim wins inside a rule
                // too.
                proposal.members.retain(|e| !claimed.contains(&e.path));
                if let Some(group) = settle(scan, proposal, &mut claimed, &mut untouched) {
                    groups.push(group);
                }
            }
        }
        // Everything still unclaimed is honestly reported as ungrouped.
        for e in candidates
            .iter()
            .map(|f| f.entry)
            .filter(|e| !claimed.contains(&e.path))
        {
            untouched.push((e.path.clone(), Untouched::NoClearGroup));
        }
        Claimed { groups, untouched }
    }
}

/// One folder rule. A rule is a pure function of the candidates and the claim
/// set: it reads facts and returns proposed groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rule<'m> {
    /// The person's saved existing-folder routes.
    SavedRoutes(&'m [MapSpec]),
    ScreenCaptures,
    CameraBurst,
    Installers,
    TypeFamilies,
    /// `--map` routes an agent supplied for this run.
    OneShotMaps(&'m [MapSpec]),
}

impl<'m> Rule<'m> {
    /// Where this rule sits in `DECISION_PRECEDENCE`.
    ///
    /// A saved route claims at the built-in tier, ahead of sweep's own rules:
    /// the person chose that folder for that extension before this run, so it
    /// is not a guess, and it is not an agent's mapping either. It still runs
    /// after the gate, like every rule.
    #[cfg(test)]
    fn class(&self) -> crate::plan::DecisionClass {
        use crate::plan::DecisionClass;
        match self {
            Rule::SavedRoutes(_)
            | Rule::ScreenCaptures
            | Rule::CameraBurst
            | Rule::Installers
            | Rule::TypeFamilies => DecisionClass::BuiltInGrouping,
            Rule::OneShotMaps(_) => DecisionClass::AgentMapping,
        }
    }

    /// The groups this rule proposes from the candidates not yet claimed.
    fn propose<'a>(&self, candidates: &[&Facts<'a>], claimed: &ClaimSet) -> Vec<Proposal<'a, 'm>> {
        let open = || {
            candidates
                .iter()
                .filter(|f| !claimed.contains(&f.entry.path))
        };
        let mut proposals = match *self {
            // A saved profile is an explicit user taxonomy. It may claim
            // files that a built-in grouping would otherwise take, but only
            // after the gate has removed every personal, project, in-flight,
            // and too-recent refusal. The CLI checked each destination: an
            // existing direct folder keeps `created: false`, which tells
            // apply never to create it if it disappears; a missing one is
            // `created: true` and apply makes it for the first file that
            // moves. A route is the person's own choice, not a guess, so it
            // has no floor: one matching file moves, and a missing folder is
            // made for it. A routed extension's files never fall to a
            // built-in group.
            Rule::SavedRoutes(routes) => routes
                .iter()
                .map(|route| Proposal {
                    folder: route.folder.clone(),
                    basis: Basis::SavedRoute(route),
                    members: open()
                        .map(|f| f.entry)
                        .filter(|e| e.ext.eq_ignore_ascii_case(&route.ext))
                        .collect(),
                })
                .collect(),
            // Structural detectors, highest precision first.
            Rule::ScreenCaptures => vec![Proposal {
                folder: "Screenshots".to_string(),
                basis: Basis::Screenshot,
                members: open()
                    .filter(|f| f.capture.is_some())
                    .map(|f| f.entry)
                    .collect(),
            }],
            Rule::CameraBurst => {
                let burst = camera_burst(open().filter(|f| f.camera).map(|f| f.entry).collect());
                let folder = match date_range(&burst) {
                    Some(r) => format!("Photos, {r}"),
                    None => "Photos".to_string(),
                };
                vec![Proposal {
                    folder,
                    basis: Basis::CameraBurst,
                    members: burst,
                }]
            }
            Rule::Installers => vec![Proposal {
                folder: "Installers".to_string(),
                basis: Basis::Installer,
                members: open().filter(|f| f.installer).map(|f| f.entry).collect(),
            }],
            // There is deliberately no rule grouping by shared words. One
            // existed: any token in five or more filenames became a folder
            // named after that token, guarded by a hand-written stoplist. On
            // the first real Downloads folder it met, it made a folder called
            // "apple" out of a receipt, an agreement, a script and an export
            // that shared a word, and the stoplist could never have saved it
            // -- the set of words that are not categories is the whole
            // vocabulary minus a few dozen. Frequency is not category.
            //
            // Its lifetime record on data this repo did not author: zero true
            // positives, one false positive. Both showcase groups in the
            // fixture were planted to demonstrate it. The one thing it was
            // right about -- a project cluster spanning extensions -- is
            // caught by a better, observable signal: a project file marks its
            // folder as one unit.
            //
            // Two independent reviews converged here: one flagged the
            // stoplist as limited, the other watched the rule produce
            // "apple". Do not reintroduce a token rule; extend the structural
            // detectors instead.

            // Type families. A file's extension says what it IS, which is the
            // question a folder name should answer. Extensions the table does
            // not know are left alone rather than swept into an "Other"
            // drawer. The table is the whole mechanism -- the OS is not
            // consulted -- and unknown means untouched, because filing formats
            // sweep cannot name would mean maintaining a list of every app's
            // private extension forever.
            //
            // Keyed by folder name, so the groups come out in that name's
            // order, as they always have.
            Rule::TypeFamilies => {
                let mut by_family: BTreeMap<&'static str, Vec<&Entry>> = BTreeMap::new();
                for f in open() {
                    if let Some(family) = f.family {
                        by_family
                            .entry(family.folder_name())
                            .or_default()
                            .push(f.entry);
                    }
                }
                by_family
                    .into_iter()
                    .map(|(folder, members)| Proposal {
                        folder: folder.to_string(),
                        basis: Basis::Family,
                        members,
                    })
                    .collect()
            }
            // Agent mappings, last and least: they may claim only what every
            // built-in rule declined. A mapping can never take a file from a
            // detector, and nothing here runs before the refusals -- a
            // personal record, a held document's reference surface, a fresh
            // file or an in-flight download never reaches this point. A --map
            // folder that sweep would have to make keeps the three-file floor
            // of every structural group: below it, --map would be an
            // arbitrary one-file move command wearing a safety vocabulary.
            // Into a folder that is already here, any number moves, the same
            // as a built-in group.
            Rule::OneShotMaps(maps) => maps
                .iter()
                .map(|map| Proposal {
                    folder: map.folder.clone(),
                    basis: Basis::Map(map),
                    members: open()
                        .map(|f| f.entry)
                        .filter(|e| e.ext.eq_ignore_ascii_case(&map.ext))
                        .collect(),
                })
                .collect(),
        };
        // A proposal with no file can form no group and leaves no file in
        // place, so it is not proposed.
        proposals.retain(|p| !p.members.is_empty());
        proposals
    }
}

/// Window within which camera files count as one burst.
const BURST_DAYS: u32 = 3;

/// The largest set of camera files taken within `BURST_DAYS` of each other.
/// Files without a readable time cannot be placed in a window and are left
/// out.
fn camera_burst(mut camera: Vec<&Entry>) -> Vec<&Entry> {
    camera.retain(|e| e.modified.is_some());
    camera.sort_by_key(|e| e.modified);
    let burst_window = Duration::from_secs(u64::from(BURST_DAYS) * 86_400);
    let mut window_start = 0;
    let mut best_start = 0;
    let mut best_len = 0;
    for window_end in 0..camera.len() {
        while camera[window_end]
            .modified
            .unwrap()
            .duration_since(camera[window_start].modified.unwrap())
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
    camera[best_start..best_start + best_len].to_vec()
}

/// A group a rule proposes, before `settle` decides which files move.
struct Proposal<'a, 'm> {
    /// The destination folder, directly under the root.
    folder: String,
    basis: Basis<'m>,
    members: Vec<&'a Entry>,
}

/// Why a proposed group exists. It becomes the group's `Signal` once `settle`
/// knows which files move.
enum Basis<'m> {
    Screenshot,
    CameraBurst,
    Installer,
    Family,
    SavedRoute(&'m MapSpec),
    Map(&'m MapSpec),
}

impl Basis<'_> {
    /// A saved route names the person's own folder, so it has no floor, and
    /// its folder counts as here even when sweep will make it.
    fn chosen_by_person(&self) -> bool {
        matches!(self, Basis::SavedRoute(_))
    }

    /// The signal for the files that move. A type family names the
    /// extensions of those files, not of the files already in its folder.
    fn signal(&self, moving: &[&Entry]) -> Signal {
        match self {
            Basis::Screenshot => Signal::Screenshot,
            Basis::CameraBurst => Signal::CameraBurst { days: BURST_DAYS },
            Basis::Installer => Signal::Installer,
            Basis::Family => Signal::TypeFamily {
                exts: moving.iter().map(|e| e.ext.to_lowercase()).collect(),
            },
            Basis::SavedRoute(route) => Signal::Mapped {
                ext: route.ext.clone(),
                created: route.created,
                saved: true,
            },
            Basis::Map(map) => Signal::Mapped {
                ext: map.ext.clone(),
                created: map.created,
                saved: false,
            },
        }
    }
}

/// The files a rule has already taken. A later rule never takes one of them:
/// the first rule to claim a file wins, which is what makes the rule order
/// mean something.
///
/// A set, not a list: every rule asks it once per remaining file, and a list
/// made that cost grow with the files times the files already claimed.
#[derive(Debug, Default)]
struct ClaimSet(HashSet<PathBuf>);

impl ClaimSet {
    fn contains(&self, path: &Path) -> bool {
        self.0.contains(path)
    }

    fn claim(&mut self, path: &Path) {
        self.0.insert(path.to_path_buf());
    }

    fn claim_all<'e>(&mut self, entries: impl IntoIterator<Item = &'e Entry>) {
        self.0.extend(entries.into_iter().map(|e| e.path.clone()));
    }
}

/// Minimum members before sweep makes a NEW folder from its own rule. Below
/// this, the grouping is a guess from too few files and the honest answer is
/// "no clear group". The floor guards folder creation only: when the
/// destination folder is already a direct child of the root, or a saved route
/// names it, any number of files moves into it.
const MIN_STRUCTURAL_GROUP: usize = 3;

/// True when `name` is a real directory directly under the root, as the scan
/// snapshot observed it. Apply checks the same observation before each move
/// into it, so a folder that is gone by then is refused, never re-created for
/// a group the floor would not have formed. No snapshot means not observed.
fn folder_observed(scan: &ScanOutcome, name: &str) -> bool {
    let Some(snapshot) = &scan.snapshot else {
        return false;
    };
    let committed = crate::scan::TreeSnapshot::path_commitment(Path::new(name));
    snapshot
        .entries
        .iter()
        .any(|entry| entry.path == committed && entry.identity.kind == 2)
}

/// Settle one proposed group. When its folder is already here, or a saved
/// route names it, files directly inside it stay where they are (they are
/// already home) and any number of the rest moves in. When sweep would have
/// to make the folder, the floor applies. Returns the group of files to move,
/// or `None` when there is no group; only the moved files and the in-place
/// files are claimed.
fn settle(
    scan: &ScanOutcome,
    proposal: Proposal<'_, '_>,
    claimed: &mut ClaimSet,
    untouched: &mut Vec<(PathBuf, Untouched)>,
) -> Option<Group> {
    let Proposal {
        folder,
        basis,
        members: candidates,
    } = proposal;
    let members = if basis.chosen_by_person() || folder_observed(scan, &folder) {
        let destination = scan.root.join(&folder);
        let (in_place, members): (Vec<&Entry>, Vec<&Entry>) = candidates
            .into_iter()
            .partition(|e| e.path.parent() == Some(destination.as_path()));
        for entry in in_place {
            claimed.claim(&entry.path);
            untouched.push((entry.path.clone(), Untouched::AlreadyInDestination));
        }
        if members.is_empty() {
            return None;
        }
        members
    } else {
        if candidates.len() < MIN_STRUCTURAL_GROUP {
            return None;
        }
        candidates
    };
    claimed.claim_all(members.iter().copied());
    Some(Group {
        name: folder,
        signal: basis.signal(&members),
        members: members.iter().map(|e| e.path.clone()).collect(),
        accepted: false,
    })
}

/// The tag mode's claim step. Reads each taggable file's tag attribute (never
/// its contents) to learn which tags exist in the folder and which files
/// already carry the one asked for. Writes nothing.
///
/// A tag moves nothing, so where a file sits has no bearing on whether it can
/// be tagged: no folder rule runs here. Every file the gate lets through can
/// be tagged, and nothing in a tag plan is "in place". A file whose tag state
/// cannot be parsed is listed as unreadable and given nothing.
fn tag(passed: &[&Facts], specs: &[TagSpec]) -> Claimed {
    let mut untouched = Vec::new();
    let mut candidates: Vec<&Entry> = passed
        .iter()
        .map(|f| f.entry)
        .filter(|entry| !entry.is_dir)
        .collect();
    candidates.sort_by(|a, b| a.path.cmp(&b.path));

    // Tag state of every file that could be tagged. `None`: unknown.
    let states: Vec<Option<Vec<String>>> = candidates
        .iter()
        .map(|entry| match crate::tags::read(&entry.path) {
            Ok(None) => Some(Vec::new()),
            Ok(Some(raw)) => crate::tags::decode(&raw),
            Err(_) => None,
        })
        .collect();
    // Tags already on those files, by lower-cased name, keeping the first
    // stored spelling (with its colour) so a new use matches the old one.
    let mut known: BTreeMap<String, String> = BTreeMap::new();
    for entry in states.iter().flatten().flatten() {
        known
            .entry(crate::tags::name_of(entry).to_lowercase())
            .or_insert_with(|| entry.clone());
    }

    let mut groups = Vec::new();
    let mut claimed = ClaimSet::default();
    for spec in specs {
        let (entry, new) = match known.get(&spec.name.to_lowercase()) {
            Some(existing) => (existing.clone(), false),
            None => (
                finder_colour_tag(&spec.name).map_or_else(|| spec.name.clone(), str::to_string),
                true,
            ),
        };
        let display = crate::tags::name_of(&entry).to_string();
        let mut members = Vec::new();
        for (candidate, state) in candidates.iter().zip(&states) {
            if !candidate.ext.eq_ignore_ascii_case(&spec.ext) {
                continue;
            }
            claimed.claim(&candidate.path);
            match state {
                None => untouched.push((candidate.path.clone(), Untouched::TagStateUnreadable)),
                Some(tags)
                    if tags
                        .iter()
                        .any(|tag| crate::tags::same_name(crate::tags::name_of(tag), &display)) =>
                {
                    untouched.push((candidate.path.clone(), Untouched::TagAlreadyPresent));
                }
                Some(_) => members.push(candidate.path.clone()),
            }
        }
        if members.is_empty() {
            continue;
        }
        groups.push(Group {
            name: display,
            signal: Signal::Tagged {
                ext: spec.ext.clone(),
                new,
                entry,
            },
            members,
            accepted: false,
        });
    }
    // What no tag claimed matched no group. Folders included: a folder is
    // never tagged, and is still reported as left alone.
    for entry in passed
        .iter()
        .map(|f| f.entry)
        .filter(|e| !claimed.contains(&e.path))
    {
        untouched.push((entry.path.clone(), Untouched::NoClearGroup));
    }
    Claimed { groups, untouched }
}

/// The seven colour tags Finder ships with, and the colour number it stores
/// after the name. Adding one of these by name must write the number, or the
/// tag would show without its colour.
fn finder_colour_tag(name: &str) -> Option<&'static str> {
    match name.to_lowercase().as_str() {
        "gray" | "grey" => Some("Gray\n1"),
        "green" => Some("Green\n2"),
        "purple" => Some("Purple\n3"),
        "blue" => Some("Blue\n4"),
        "yellow" => Some("Yellow\n5"),
        "red" => Some("Red\n6"),
        "orange" => Some("Orange\n7"),
        _ => None,
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
    use crate::plan::{DECISION_PRECEDENCE, DecisionClass};

    fn route(ext: &str, folder: &str) -> MapSpec {
        MapSpec {
            ext: ext.into(),
            folder: folder.into(),
            created: true,
        }
    }

    fn rank(class: DecisionClass) -> usize {
        DECISION_PRECEDENCE
            .iter()
            .position(|c| *c == class)
            .expect("every class is in DECISION_PRECEDENCE")
    }

    /// Each mode's rules, in claim order. The order decides which group a
    /// file joins when two rules want it.
    #[test]
    fn each_mode_lists_its_rules_in_claim_order() {
        let maps = vec![route("bpy", "Blender")];
        assert_eq!(
            Mode::Folders { maps: maps.clone() }.rules(),
            vec![
                Rule::ScreenCaptures,
                Rule::CameraBurst,
                Rule::Installers,
                Rule::TypeFamilies,
                Rule::OneShotMaps(&maps),
            ]
        );
        assert_eq!(
            Mode::Existing {
                routes: maps.clone()
            }
            .rules(),
            vec![
                Rule::SavedRoutes(&maps),
                Rule::ScreenCaptures,
                Rule::CameraBurst,
                Rule::Installers,
                Rule::TypeFamilies,
            ]
        );
        assert!(Mode::Tags { specs: Vec::new() }.rules().is_empty());
        assert_eq!(Mode::default(), Mode::Folders { maps: Vec::new() });
    }

    /// `DECISION_PRECEDENCE` holds by construction. The gate (the safety
    /// refusals) runs before any rule, so every rule ranks below it; and in
    /// each mode no rule outranks one before it, so the built-in rules come
    /// before any agent mapping.
    #[test]
    fn every_mode_claims_in_decision_precedence_order() {
        let maps = vec![route("bpy", "Blender")];
        let modes = [
            Mode::Folders { maps: maps.clone() },
            Mode::Existing {
                routes: maps.clone(),
            },
            Mode::Tags { specs: Vec::new() },
        ];
        let gate = rank(DecisionClass::SafetyRefusal);
        for mode in &modes {
            let ranks: Vec<usize> = mode.rules().iter().map(|r| rank(r.class())).collect();
            assert!(
                ranks.iter().all(|r| *r > gate),
                "{mode:?}: a rule ranks with or above the gate: {ranks:?}"
            );
            assert!(
                ranks.windows(2).all(|pair| pair[0] <= pair[1]),
                "{mode:?}: a rule claims before one that outranks it: {ranks:?}"
            );
        }
        let folders = modes[0].rules();
        assert_eq!(
            folders.last().map(Rule::class),
            Some(DecisionClass::AgentMapping),
            "--map claims last"
        );
        assert!(
            folders[..folders.len() - 1]
                .iter()
                .all(|r| r.class() == DecisionClass::BuiltInGrouping),
            "every rule before --map is a built-in rule"
        );
    }

    #[test]
    fn date_range_does_not_collapse_a_multi_year_span_to_one_date() {
        let at = |name: &str, secs: u64| Entry {
            path: PathBuf::from(name),
            name: name.to_string(),
            ext: "jpg".into(),
            size: 1,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
            is_dir: false,
            is_package: false,
        };
        let a = at("a.jpg", 1_577_836_800); // 2020-01-01
        let b = at("b.jpg", 1_767_225_600); // 2026-01-01
        let range = date_range(&[&a, &b]).expect("both entries have modified times");
        assert_ne!(
            range, "Jan 1",
            "multi-year range collapsed to a single date"
        );
        assert_eq!(range, "Jan 1, 2020–Jan 1, 2026");
    }
}
