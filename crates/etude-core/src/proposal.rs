//! Per-file proposals: a caller decides where individual files go, sweep checks
//! every decision against its own scan, and the result is a new bound plan.
//!
//! The rule that matters: **eligibility comes from the plan, never from the
//! proposal.** The eligible list is derived here from the plan's own groups and
//! its recorded unclaimed files. Both are covered by the plan digest, so an
//! edited plan is refused when it loads. A proposal can only point at a place
//! in that list. It cannot add a file to it, and a personal-looking file, a
//! too-recent file, a download in flight, a file in a project folder, a
//! symlink or a hidden entry is not in it, so there is no id to name.
//!
//! Nothing here reads file contents. Nothing here moves a file: the output is
//! a plan, applied by the ordinary `sweep apply --plan`.

use crate::Untouched;
use crate::json as j;
use crate::plan::{BoundPlan, Group, Signal};
use crate::scan::TreeSnapshot;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const MAX_PROPOSAL_BYTES: u64 = 1024 * 1024;
pub const MAX_DECISIONS: usize = 5_000;
const MAX_JSON_DEPTH: usize = 8;
const MAX_FOLDER_BYTES: usize = 255;

/// One file a proposal may name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EligibleItem {
    /// `i-000017`: an index into this plan's eligible list. Meaningless
    /// outside the plan whose digest it is bound to.
    pub id: String,
    pub name: String,
    pub size: u64,
    /// The group the plan already proposes for it, or `None` when no group
    /// claimed it.
    pub group: Option<String>,
    path: PathBuf,
}

impl EligibleItem {
    pub fn json(&self) -> String {
        j::obj(&[
            ("id", j::str(&self.id)),
            ("name", j::str(&self.name)),
            ("size", j::num(self.size)),
            (
                "group",
                self.group
                    .as_deref()
                    .map(j::str)
                    .unwrap_or_else(|| "null".into()),
            ),
        ])
    }
}

fn item_id(index: usize) -> String {
    format!("i-{index:06}")
}

/// The files the plan proposes to move, in group order, then the files left
/// alone only because no group claimed them. Derived from the plan alone.
pub fn eligible_items(plan: &BoundPlan) -> Vec<EligibleItem> {
    let mut items = Vec::new();
    let mut push = |path: &Path, group: Option<&str>| {
        items.push(EligibleItem {
            id: item_id(items.len()),
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            size: plan.observed_file_size(path).unwrap_or(0),
            group: group.map(str::to_owned),
            path: path.to_path_buf(),
        });
    };
    for group in &plan.groups {
        for member in &group.members {
            push(member, Some(&group.name));
        }
    }
    for path in &plan.unclaimed {
        push(path, None);
    }
    items
}

pub fn items_json(items: &[EligibleItem]) -> String {
    j::arr(items.iter().map(EligibleItem::json))
}

/// Why a proposal was refused. One bad decision refuses the whole proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Size, count, JSON, version, `rest` or decision shape.
    InvalidArguments(String),
    /// The proposal names another plan, or the plan or tree changed.
    PlanStale(String),
    /// An item id is not in the plan's eligible list.
    UnknownItem(String),
    /// An item is named more than once.
    ItemAssignedTwice(String),
    /// A folder is not one plain name.
    DestinationOutsideRoot(String),
    /// The folder does not exist.
    ProfileMissing(String),
    /// A non-folder holds the folder's name, or the file's name is taken.
    DestinationExists(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (Self::InvalidArguments(m)
        | Self::PlanStale(m)
        | Self::UnknownItem(m)
        | Self::ItemAssignedTwice(m)
        | Self::DestinationOutsideRoot(m)
        | Self::ProfileMissing(m)
        | Self::DestinationExists(m)) = self;
        f.write_str(m)
    }
}

/// A checked proposal: a new bound plan, and what it does.
#[derive(Debug)]
pub struct Proposed {
    pub plan: BoundPlan,
    pub moves: usize,
    pub skipped: usize,
}

enum Decision {
    Skip { item: String },
    Move { item: String, folder: String },
}

struct Parsed {
    digest: String,
    decisions: Vec<Decision>,
}

fn invalid(message: impl Into<String>) -> Refusal {
    Refusal::InvalidArguments(message.into())
}

/// Check a proposal against a loaded plan and build the plan it describes.
///
/// `plan` must already have been loaded with its digest (`BoundPlan::load`),
/// which is what makes the eligible list trustworthy.
pub fn propose(plan: &BoundPlan, proposal: &[u8]) -> Result<Proposed, Refusal> {
    if plan.context().tool != "sweep" {
        return Err(invalid("per-file proposals apply to sweep plans only"));
    }
    if plan.applies_tags() {
        return Err(invalid(
            "a tags plan adds tags and places no files; proposals place files in folders",
        ));
    }
    let parsed = parse(proposal)?;
    if parsed.digest != plan.digest() {
        return Err(Refusal::PlanStale(
            "the proposal was built from a different plan; export a fresh plan".into(),
        ));
    }
    let items = eligible_items(plan);
    let root = plan.root.clone();
    let mut seen: HashSet<usize> = HashSet::new();
    let mut checked_folders: HashSet<String> = HashSet::new();
    let mut taken: HashSet<(String, String)> = HashSet::new();
    let mut order: Vec<String> = Vec::new();
    let mut by_folder: HashMap<String, Vec<usize>> = HashMap::new();
    let mut skipped = 0usize;
    for decision in &parsed.decisions {
        let item = match decision {
            Decision::Skip { item } | Decision::Move { item, .. } => item,
        };
        let index = parse_item_id(item)
            .filter(|index| *index < items.len())
            .ok_or_else(|| {
                Refusal::UnknownItem(format!("item {item:?} is not in this plan's eligible list"))
            })?;
        if !seen.insert(index) {
            return Err(Refusal::ItemAssignedTwice(format!(
                "item {item:?} is named more than once"
            )));
        }
        let Decision::Move { folder, .. } = decision else {
            skipped += 1;
            continue;
        };
        check_folder_name(folder)?;
        if checked_folders.insert(folder.clone()) {
            check_folder_exists(plan, &root, folder)?;
        }
        let entry = &items[index];
        let target = root.join(folder).join(&entry.name);
        if std::fs::symlink_metadata(&target).is_ok() {
            return Err(Refusal::DestinationExists(format!(
                "{:?} already exists in {folder:?}; a file keeps its name",
                entry.name
            )));
        }
        if !taken.insert((folder.to_lowercase(), entry.name.to_lowercase())) {
            return Err(Refusal::DestinationExists(format!(
                "two items would both become {:?} in {folder:?}",
                entry.name
            )));
        }
        if !by_folder.contains_key(folder) {
            order.push(folder.clone());
        }
        by_folder.entry(folder.clone()).or_default().push(index);
    }

    let moves: usize = by_folder.values().map(Vec::len).sum();
    let moved: HashSet<usize> = by_folder.values().flatten().copied().collect();
    let mut next = plan.plan.clone();
    next.groups = order
        .iter()
        .map(|folder| {
            let members: Vec<PathBuf> = by_folder[folder]
                .iter()
                .map(|index| items[*index].path.clone())
                .collect();
            Group {
                name: folder.clone(),
                signal: Signal::Proposed {
                    count: members.len(),
                },
                members,
                accepted: false,
            }
        })
        .collect();
    // Everything not moved stays where it is and stays eligible. A file that
    // had a group in the source plan is, in this plan, left alone because no
    // group claims it.
    next.unclaimed = items
        .iter()
        .enumerate()
        .filter(|(index, _)| !moved.contains(index))
        .map(|(_, item)| item.path.clone())
        .collect();
    let moved_commitments: HashSet<PathBuf> = items
        .iter()
        .enumerate()
        .filter(|(index, _)| moved.contains(index))
        .map(|(_, item)| TreeSnapshot::path_commitment(&item.path))
        .collect();
    next.untouched.retain(|(path, reason)| {
        !(*reason == Untouched::NoClearGroup
            && moved_commitments.contains(&TreeSnapshot::path_commitment(path)))
    });
    let already: HashSet<PathBuf> = next
        .untouched
        .iter()
        .filter(|(_, reason)| *reason == Untouched::NoClearGroup)
        .map(|(path, _)| TreeSnapshot::path_commitment(path))
        .collect();
    for path in &next.unclaimed {
        if !already.contains(&TreeSnapshot::path_commitment(path)) {
            next.untouched.push((path.clone(), Untouched::NoClearGroup));
        }
    }
    next.untouched
        .sort_by_key(|(path, _)| TreeSnapshot::path_commitment(path));
    let bound = BoundPlan::from_plan(next, plan.context().clone())
        .map_err(|error| Refusal::PlanStale(format!("{error}; export a fresh plan")))?;
    Ok(Proposed {
        plan: bound,
        moves,
        skipped,
    })
}

fn parse_item_id(text: &str) -> Option<usize> {
    let digits = text.strip_prefix("i-")?;
    if digits.len() < 6 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let index: usize = digits.parse().ok()?;
    (item_id(index) == text).then_some(index)
}

/// One plain, visible name: no separator, no `.` or `..`, not absolute, not
/// empty. The same rule `--map` applies to its folder names.
fn check_folder_name(folder: &str) -> Result<(), Refusal> {
    if folder.is_empty()
        || folder == "."
        || folder == ".."
        || folder.contains('/')
        || folder.contains('\\')
        || folder.contains('\0')
        || folder.starts_with('.')
        || folder.len() > MAX_FOLDER_BYTES
    {
        return Err(Refusal::DestinationOutsideRoot(format!(
            "{folder:?} is not one plain folder name directly under the plan root"
        )));
    }
    Ok(())
}

/// `require_existing`: the folder is there now, and it is the same folder the
/// plan observed. Its identity is then checked again by apply.
fn check_folder_exists(plan: &BoundPlan, root: &Path, folder: &str) -> Result<(), Refusal> {
    let path = root.join(folder);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Refusal::ProfileMissing(format!(
                "folder {folder:?} does not exist; this proposal never creates one"
            )));
        }
        Err(error) => {
            return Err(Refusal::PlanStale(format!(
                "folder {folder:?} cannot be checked ({error}); export a fresh plan"
            )));
        }
    };
    if !metadata.is_dir() {
        return Err(Refusal::DestinationExists(format!(
            "{folder:?} exists but is not a folder"
        )));
    }
    #[cfg(unix)]
    let current = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let current = (0, 0);
    if plan.observed_directory_identity(&path) != Some(current) {
        return Err(Refusal::PlanStale(format!(
            "folder {folder:?} was not in the plan's observation, or was replaced; export a fresh plan"
        )));
    }
    Ok(())
}

fn parse(bytes: &[u8]) -> Result<Parsed, Refusal> {
    if bytes.len() as u64 > MAX_PROPOSAL_BYTES {
        return Err(invalid("the proposal file is larger than 1 MiB"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("the proposal is not UTF-8"))?;
    let value = Reader::new(text).document()?;
    let Json::Object(fields) = value else {
        return Err(invalid("the proposal must be a JSON object"));
    };
    let mut digest = None;
    let mut rest = None;
    let mut version = None;
    let mut decisions = None;
    for (key, value) in fields {
        let slot = match key.as_str() {
            "schema_version" => &mut version,
            "plan_digest" => &mut digest,
            "rest" => &mut rest,
            "decisions" => &mut decisions,
            other => return Err(invalid(format!("unknown proposal field {other:?}"))),
        };
        if slot.replace(value).is_some() {
            return Err(invalid(format!("proposal field {key:?} appears twice")));
        }
    }
    match version {
        Some(Json::Number(n)) if n == "1" => {}
        _ => return Err(invalid("schema_version must be 1")),
    }
    match rest {
        Some(Json::String(value)) if value == "leave" => {}
        _ => return Err(invalid("rest must be present and equal \"leave\"")),
    }
    let Some(Json::String(digest)) = digest else {
        return Err(invalid("plan_digest must be a string"));
    };
    let Some(Json::Array(list)) = decisions else {
        return Err(invalid("decisions must be a list"));
    };
    if list.len() > MAX_DECISIONS {
        return Err(invalid("a proposal holds at most 5,000 decisions"));
    }
    let mut decisions = Vec::with_capacity(list.len());
    for entry in list {
        decisions.push(decision(entry)?);
    }
    Ok(Parsed { digest, decisions })
}

fn decision(value: Json) -> Result<Decision, Refusal> {
    let Json::Object(fields) = value else {
        return Err(invalid("a decision must be an object"));
    };
    let mut item = None;
    let mut folder = None;
    let mut mode = None;
    let mut skip = None;
    for (key, value) in fields {
        let slot = match key.as_str() {
            "item" => &mut item,
            "folder" => &mut folder,
            "mode" => &mut mode,
            "skip" => &mut skip,
            other => return Err(invalid(format!("unknown decision field {other:?}"))),
        };
        if slot.replace(value).is_some() {
            return Err(invalid(format!("decision field {key:?} appears twice")));
        }
    }
    let Some(Json::String(item)) = item else {
        return Err(invalid("a decision needs an item id"));
    };
    match (folder, mode, skip) {
        (None, None, Some(Json::Bool(true))) => Ok(Decision::Skip { item }),
        (Some(Json::String(folder)), Some(Json::String(mode)), None)
            if mode == "require_existing" =>
        {
            Ok(Decision::Move { item, folder })
        }
        _ => Err(invalid(
            "a decision is either {item, skip: true} or {item, folder, mode: \"require_existing\"}",
        )),
    }
}

// A strict JSON reader. The library has no dependencies, so it reads the one
// small document it needs itself. It accepts only JSON, bounds its depth, and
// refuses what it does not understand.

#[derive(Debug)]
enum Json {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            at: 0,
        }
    }

    fn document(mut self) -> Result<Json, Refusal> {
        let value = self.value(0)?;
        self.space();
        if self.at != self.bytes.len() {
            return Err(self.fail("data after the JSON document"));
        }
        Ok(value)
    }

    fn fail(&self, what: &str) -> Refusal {
        invalid(format!(
            "the proposal is not valid JSON: {what} at byte {}",
            self.at
        ))
    }

    fn space(&mut self) {
        while matches!(self.bytes.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> Result<(), Refusal> {
        if self.bytes.get(self.at) == Some(&byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(self.fail(&format!("expected {:?}", byte as char)))
        }
    }

    fn word(&mut self, word: &str, value: Json) -> Result<Json, Refusal> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.fail("unknown literal"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, Refusal> {
        if depth > MAX_JSON_DEPTH {
            return Err(self.fail("nesting is too deep"));
        }
        self.space();
        match self.bytes.get(self.at) {
            Some(b'{') => {
                self.at += 1;
                let mut fields = Vec::new();
                self.space();
                if self.bytes.get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Ok(Json::Object(fields));
                }
                loop {
                    self.space();
                    let key = self.string()?;
                    self.space();
                    self.eat(b':')?;
                    fields.push((key, self.value(depth + 1)?));
                    self.space();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {
                            self.at += 1;
                            return Ok(Json::Object(fields));
                        }
                        _ => return Err(self.fail("expected ',' or '}'")),
                    }
                }
            }
            Some(b'[') => {
                self.at += 1;
                let mut list = Vec::new();
                self.space();
                if self.bytes.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(Json::Array(list));
                }
                loop {
                    list.push(self.value(depth + 1)?);
                    self.space();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            return Ok(Json::Array(list));
                        }
                        _ => return Err(self.fail("expected ',' or ']'")),
                    }
                }
            }
            Some(b'"') => Ok(Json::String(self.string()?)),
            Some(b't') => self.word("true", Json::Bool(true)),
            Some(b'f') => self.word("false", Json::Bool(false)),
            Some(b'n') => self.word("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(self.fail("unexpected character")),
        }
    }

    fn number(&mut self) -> Result<Json, Refusal> {
        let start = self.at;
        if self.bytes.get(self.at) == Some(&b'-') {
            self.at += 1;
        }
        match self.bytes.get(self.at) {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => self.digits(),
            _ => return Err(self.fail("malformed number")),
        }
        if self.bytes.get(self.at) == Some(&b'.') {
            self.at += 1;
            if !self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
                return Err(self.fail("malformed number"));
            }
            self.digits();
        }
        if matches!(self.bytes.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.bytes.get(self.at), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if !self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
                return Err(self.fail("malformed number"));
            }
            self.digits();
        }
        Ok(Json::Number(
            String::from_utf8_lossy(&self.bytes[start..self.at]).into_owned(),
        ))
    }

    fn digits(&mut self) {
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
    }

    fn hex4(&mut self) -> Result<u32, Refusal> {
        let digits = self
            .bytes
            .get(self.at..self.at + 4)
            .and_then(|digits| std::str::from_utf8(digits).ok())
            .filter(|digits| digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| self.fail("malformed \\u escape"))?;
        let value =
            u32::from_str_radix(digits, 16).map_err(|_| self.fail("malformed \\u escape"))?;
        self.at += 4;
        Ok(value)
    }

    fn string(&mut self) -> Result<String, Refusal> {
        self.eat(b'"')?;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(&byte) = self.bytes.get(self.at) else {
                return Err(self.fail("unterminated string"));
            };
            self.at += 1;
            match byte {
                b'"' => break,
                0x00..=0x1f => return Err(self.fail("control character in string")),
                b'\\' => {
                    let Some(&escape) = self.bytes.get(self.at) else {
                        return Err(self.fail("unterminated string"));
                    };
                    self.at += 1;
                    let c = match escape {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{08}',
                        b'f' => '\u{0c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let first = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&first) {
                                if self.bytes.get(self.at..self.at + 2) != Some(b"\\u") {
                                    return Err(self.fail("unpaired surrogate"));
                                }
                                self.at += 2;
                                let second = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&second) {
                                    return Err(self.fail("unpaired surrogate"));
                                }
                                0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
                            } else {
                                first
                            };
                            char::from_u32(code).ok_or_else(|| self.fail("invalid \\u escape"))?
                        }
                        _ => return Err(self.fail("unknown escape")),
                    };
                    let mut buffer = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
                }
                other => out.push(other),
            }
        }
        String::from_utf8(out).map_err(|_| self.fail("string is not UTF-8"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_ids_have_one_spelling() {
        assert_eq!(parse_item_id("i-000017"), Some(17));
        for bad in [
            "i-17",
            "I-000017",
            "i-0000017",
            "i-00001x",
            "17",
            "i-",
            "i--00017",
        ] {
            assert_eq!(parse_item_id(bad), None, "{bad}");
        }
        assert_eq!(parse_item_id("i-1000000"), Some(1_000_000));
    }

    #[test]
    fn folder_names_are_one_plain_visible_component() {
        for ok in ["Reading", "Taxes-2025", "a b"] {
            assert!(check_folder_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "", ".", "..", "a/b", "/abs", "../x", ".hidden", "a\\b", "a\0b",
        ] {
            assert!(check_folder_name(bad).is_err(), "{bad:?}");
        }
        assert!(check_folder_name(&"x".repeat(256)).is_err());
    }

    #[test]
    fn the_reader_accepts_json_and_refuses_the_rest() {
        let document = |text: &str| Reader::new(text).document();
        assert!(document(r#"{"a":[1,-2.5e3,true,null,"xé😀"]}"#).is_ok());
        for bad in [
            "",
            "{",
            "[1,]",
            r#"{"a":1,}"#,
            "01",
            "1.",
            "'a'",
            r#""\x""#,
            r#""\ud800""#,
            "{} {}",
            "nul",
            "\"a\nb\"",
        ] {
            assert!(document(bad).is_err(), "{bad:?}");
        }
        let deep = "[".repeat(40) + &"]".repeat(40);
        assert!(document(&deep).is_err());
    }
}
