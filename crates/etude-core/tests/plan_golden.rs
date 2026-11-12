//! Golden files for plan building: every public entry point, run on a set of
//! synthetic folders, must produce the same plan it produced when the files in
//! `tests/golden/` were written.
//!
//! This pins the planner's behaviour before its passes are taken apart. A
//! change that only moves code must leave every file here byte for byte the
//! same; a change that means to alter a plan rewrites the files on purpose and
//! says why (CI reads a `Golden-Change:` trailer, see
//! `scripts/check-golden-frozen.py`).
//!
//! Two files per folder and entry point:
//!
//! - `<fixture>__<mode>.json`: `Plan::to_json`, the serializer `--json` uses,
//!   indented one value per line so a difference reads as a line diff.
//! - `<fixture>__<mode>.bin`: the bytes an exported plan holds, encoded with a
//!   fixed `BindingContext`, without the trailing digest.
//!
//! What is normalised, and why. Both files name the temp folder each run
//! makes, so its path becomes `$ROOT` (and a path commitment of an absolute
//! path is recomputed for the `$ROOT` spelling). The exported bytes also hold
//! facts no test can pin: device and inode numbers, change times, and the
//! project markers that happen to sit above the temp folder on this machine.
//! The first three are written as zero and the ancestor list as empty. Every
//! modification time and permission is set by the fixture itself, so those
//! stay as observed.
//!
//! macOS only, like the product: the fixtures use Finder tags, Finder aliases
//! and the immutable flag, and APFS folder sizes are in the exported bytes.
//!
//! Regenerate after a deliberate change:
//!
//!   SWEEP_GOLDEN_BLESS=1 cargo test -p etude-core --test plan_golden
#![cfg(target_os = "macos")]

use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use etude_core::plan::{self, BindingContext, BoundPlan, Inspector, Plan};
use etude_core::scan::{self, ScanConfig, ScanOutcome, TreeSnapshot};
use etude_core::{Category, Untouched};

const BLESS: &str = "SWEEP_GOLDEN_BLESS";
const ROOT_TOKEN: &str = "$ROOT";

/// 2020-09-13. The default modification time of every fixture file: far
/// enough in the past that no grace window used here reaches it.
const T0: u64 = 1_600_000_000;
const HOUR: u64 = 3_600;
const DAY: u64 = 86_400;
/// 1970-01-02. Older than the fifty-year window the recency fixtures use.
const ANCIENT: u64 = DAY;
/// 2100-01-01. A modification time in the future: with a window in force it
/// counts as too recent until that date, so the result does not drift.
const FUTURE: u64 = 4_102_444_800;
/// Wide enough that T0 is inside it and ANCIENT is not, until 2070.
const FIFTY_YEARS: Duration = Duration::from_secs(50 * 365 * DAY);

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// The context every exported plan here is bound to. Fixed, so the bytes do
/// not change with the crate version.
fn context() -> BindingContext {
    BindingContext::new("sweep", "golden", "golden-profile", "golden-contract")
}

/// Content reading that refuses one name, so the modes that take an
/// inspector differ from the ones that do not.
struct GoldenInspector;
impl Inspector for GoldenInspector {
    fn inspect(&mut self, path: &Path, _ext: &str) -> Option<Category> {
        let name = path.file_name()?.to_string_lossy();
        name.contains("bloodwork").then_some(Category::Medical)
    }
}

// --- building a folder ------------------------------------------------------

unsafe extern "C" {
    fn chflags(path: *const std::ffi::c_char, flags: u32) -> i32;
    fn setxattr(
        path: *const std::ffi::c_char,
        name: *const std::ffi::c_char,
        value: *const std::ffi::c_void,
        size: usize,
        position: u32,
        options: i32,
    ) -> i32;
    fn lutimes(path: *const std::ffi::c_char, times: *const Timeval) -> i32;
}

#[repr(C)]
struct Timeval {
    sec: i64,
    usec: i32,
}

const UF_IMMUTABLE: u32 = 0x0000_0002;
const TAG_ATTR: &std::ffi::CStr = c"com.apple.metadata:_kMDItemUserTags";
const FINDER_INFO: &std::ffi::CStr = c"com.apple.FinderInfo";

fn c_path(path: &Path) -> CString {
    CString::new(path.as_os_str().as_bytes()).unwrap()
}

fn set_attr(path: &Path, name: &std::ffi::CStr, value: &[u8]) {
    let path = c_path(path);
    // SAFETY: valid C strings and a slice that outlives the call.
    let rc = unsafe {
        setxattr(
            path.as_ptr(),
            name.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    assert_eq!(rc, 0, "could not set a fixture attribute");
}

fn set_flags(path: &Path, flags: u32) -> i32 {
    // SAFETY: a valid C string; chflags reads it and nothing else.
    unsafe { chflags(c_path(path).as_ptr(), flags) }
}

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

fn set_mtime(path: &Path, secs: u64) {
    let time = at(secs);
    let file = fs::File::open(path).unwrap();
    file.set_times(fs::FileTimes::new().set_modified(time).set_accessed(time))
        .unwrap();
}

/// One synthetic folder in its own temp directory. Every file gets a fixed
/// mode and modification time, and every folder's time is set after its last
/// child, so a scan observes the same metadata on every run.
struct Fixture {
    base: PathBuf,
    root: PathBuf,
    dirs: Vec<PathBuf>,
    locks: Vec<PathBuf>,
}

impl Fixture {
    fn new(name: &str) -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "etude-plan-golden-{}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = base.join(name);
        fs::create_dir_all(&root).unwrap();
        let mut fixture = Self {
            base,
            root: root.clone(),
            dirs: Vec::new(),
            locks: Vec::new(),
        };
        fixture.mark_dir(root);
        fixture
    }

    fn mark_dir(&mut self, path: PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        self.dirs.push(path);
    }

    fn dir(&mut self, rel: &str) -> &mut Self {
        let path = self.root.join(rel);
        fs::create_dir(&path).unwrap();
        self.mark_dir(path);
        self
    }

    fn file_at(&mut self, rel: &str, secs: u64) -> &mut Self {
        use std::os::unix::fs::PermissionsExt;
        let path = self.root.join(rel);
        fs::write(&path, format!("synthetic {rel}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        set_mtime(&path, secs);
        self
    }

    fn file(&mut self, rel: &str) -> &mut Self {
        self.file_at(rel, T0)
    }

    fn files(&mut self, rels: &[&str]) -> &mut Self {
        for rel in rels {
            self.file(rel);
        }
        self
    }

    /// A file carrying a Finder tag attribute with these exact bytes.
    fn tagged(&mut self, rel: &str, value: &[u8]) -> &mut Self {
        self.file(rel);
        set_attr(&self.root.join(rel), TAG_ATTR, value);
        self
    }

    /// A folder carrying a Finder tag attribute.
    fn tagged_dir(&mut self, rel: &str) -> &mut Self {
        self.dir(rel);
        set_attr(&self.root.join(rel), TAG_ATTR, b"opaque-tag-bytes");
        self
    }

    /// A file Finder treats as an alias: kIsAlias, bit 15 of the big-endian
    /// flags word at byte 8 of FinderInfo.
    fn alias(&mut self, rel: &str) -> &mut Self {
        self.file(rel);
        let mut info = [0u8; 32];
        info[8] = 0x80;
        set_attr(&self.root.join(rel), FINDER_INFO, &info);
        self
    }

    fn symlink(&mut self, rel: &str, target: &str) -> &mut Self {
        let path = self.root.join(rel);
        std::os::unix::fs::symlink(target, &path).unwrap();
        let times = [
            Timeval {
                sec: T0 as i64,
                usec: 0,
            },
            Timeval {
                sec: T0 as i64,
                usec: 0,
            },
        ];
        // SAFETY: a valid C string and two timevals that outlive the call.
        let rc = unsafe { lutimes(c_path(&path).as_ptr(), times.as_ptr()) };
        assert_eq!(rc, 0, "could not set the link's time");
        self
    }

    /// Set the immutable flag once the folder is finished.
    fn locked(&mut self, rel: &str) -> &mut Self {
        self.locks.push(self.root.join(rel));
        self
    }

    /// Fix every folder's time, deepest first, then lock what was asked.
    fn finish(&mut self) {
        for dir in self.dirs.iter().rev() {
            set_mtime(dir, T0);
        }
        for path in &self.locks {
            assert_eq!(set_flags(path, UF_IMMUTABLE), 0, "could not lock");
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for path in &self.locks {
            set_flags(path, 0);
        }
        let _ = fs::remove_dir_all(&self.base);
    }
}

// --- the fixtures -----------------------------------------------------------

type Pairs = &'static [(&'static str, &'static str)];

struct Case {
    name: &'static str,
    depth: u8,
    grace: Option<Duration>,
    include_tagged: bool,
    /// One-shot `--map EXT=FOLDER` routes.
    maps: Pairs,
    /// Saved routes of an existing-folder profile.
    routes: Pairs,
    /// `--scheme tags` with `--map EXT=TAG`.
    tags: Pairs,
    build: fn(&mut Fixture),
    /// A change to the scan result that no folder on disk can produce.
    adjust: fn(&mut ScanOutcome),
}

const PLAIN: Case = Case {
    name: "",
    depth: 1,
    grace: None,
    include_tagged: false,
    maps: &[],
    routes: &[],
    tags: &[],
    build: |_| {},
    adjust: |_| {},
};

fn cases() -> Vec<Case> {
    vec![
        // The screenshot rule, in each spelling it knows, and a file whose
        // name starts the same way but whose type is not an image.
        Case {
            name: "screenshots",
            tags: &[("png", "Captures")],
            build: |f| {
                f.files(&[
                    "Screenshot 2026-01-01 at 10.00.00.png",
                    "Screenshot 2026-01-01 at 10.00.01.png",
                    "Screen Shot 2019-05-05 at 9.00.00 AM.jpg",
                    "CleanShot 2026-02-02 at 08.00.00.heic",
                    "screenshot-notes.txt",
                ]);
            },
            ..PLAIN
        },
        // Camera bursts by DCF name: four letters, digits or underscores,
        // then 0001-9999. The burst is the largest run inside three days;
        // IMG_0100 is forty days later and stays out. IMG_1040 holds a tax
        // form number, which the DCF guard does not read as personal.
        // MVI_0005 is a camera name the rule does not know.
        Case {
            name: "camera_burst",
            tags: &[("jpg", "Trip")],
            build: |f| {
                f.file_at("IMG_0001.JPG", T0)
                    .file_at("IMG_0002.JPG", T0 + HOUR)
                    .file_at("IMG_1040.JPG", T0 + 2 * HOUR)
                    .file_at("DSC_0003.JPG", T0 + DAY)
                    .file_at("DJI_0006.MOV", T0 + DAY + HOUR)
                    .file_at("PXL_0004.HEIC", T0 + 2 * DAY)
                    .file_at("IMG_0100.JPG", T0 + 40 * DAY)
                    .file_at("MVI_0005.MOV", T0 + DAY);
            },
            ..PLAIN
        },
        // Installers: .dmg and .pkg, and the other installer types the rule
        // also takes although its description names only the first two.
        Case {
            name: "installers",
            build: |f| {
                f.files(&["Setup.dmg", "Tool.pkg", "Driver.dmg", "Agent.msi"]);
            },
            ..PLAIN
        },
        // Every type family with three or more members, and extensions no
        // family knows.
        Case {
            name: "type_families",
            maps: &[("bpy", "Blends")],
            routes: &[("bpy", "Blends"), ("csv", "Sheets")],
            tags: &[("pdf", "Read"), ("py", "Code")],
            build: |f| {
                f.files(&[
                    "report.pdf",
                    "notes.md",
                    "letter.docx",
                    "build.sh",
                    "tool.py",
                    "page.js",
                    "a.zip",
                    "b.tar.gz",
                    "c.7z",
                    "song.mp3",
                    "clip.mov",
                    "voice.m4a",
                    "table.csv",
                    "sheet.xlsx",
                    "data.tsv",
                    "photo.png",
                    "art.gif",
                    "scan.tiff",
                    "model.bpy",
                    "scene.fbx",
                    "config.json",
                ]);
            },
            ..PLAIN
        },
        // Two of each: below the three-file minimum, no new folder is made.
        Case {
            name: "below_floor",
            maps: &[("bpy", "Blends")],
            build: |f| {
                f.files(&[
                    "one.pdf",
                    "two.pdf",
                    "one.zip",
                    "two.zip",
                    "One.dmg",
                    "Two.dmg",
                    "Screenshot 2026-01-01 at 10.00.00.png",
                    "Screenshot 2026-01-01 at 10.00.01.png",
                    "one.bpy",
                    "two.bpy",
                ]);
            },
            ..PLAIN
        },
        // Folders that are already here: files inside them stay
        // (AlreadyInDestination), and any number of loose files moves in,
        // below the floor.
        Case {
            name: "already_here",
            depth: 2,
            tags: &[("pdf", "Read")],
            build: |f| {
                f.dir("Documents")
                    .dir("Screenshots")
                    .dir("Installers")
                    .files(&[
                        "Documents/old.pdf",
                        "Documents/older.pdf",
                        "Installers/Old.dmg",
                        "new.pdf",
                        "Screenshot 2026-03-03 at 1.00.00.png",
                    ]);
            },
            ..PLAIN
        },
        // A loose project document: it stays, the families it could
        // reference stay, screenshots beside it are exempt, and what it could
        // not reference still sorts. The scan decides these holds, so they
        // reach the plan through `project_holds`.
        Case {
            name: "project_document",
            tags: &[("wav", "Samples"), ("pdf", "Read")],
            build: |f| {
                f.files(&[
                    "mix.flp",
                    "kick.wav",
                    "snare.wav",
                    "hat.wav",
                    "cover.png",
                    "sleeve.png",
                    "back.png",
                    "chop.py",
                    "take.csv",
                    "notes.pdf",
                    "lyrics.pdf",
                    "credits.pdf",
                    "PluginA.dmg",
                    "PluginB.dmg",
                    "PluginC.dmg",
                    "stems.zip",
                    "Screenshot 2026-01-01 at 10.00.00.png",
                    "Screenshot 2026-01-01 at 10.00.01.png",
                    "Screenshot 2026-01-01 at 10.00.02.png",
                ]);
            },
            ..PLAIN
        },
        // Personal records by name, one by the inspector only, and a camera
        // name holding a tax form number.
        Case {
            name: "personal",
            tags: &[("pdf", "Read")],
            build: |f| {
                f.files(&[
                    "tax_return_2023.pdf",
                    "passport_scan.jpg",
                    "bank_statement_march.pdf",
                    "lab_results_2024.pdf",
                    "w2_2024.pdf",
                    "IMG_1099.JPG",
                    "bloodwork-summary.pdf",
                    "agenda.pdf",
                    "menu.pdf",
                    "recipe.pdf",
                ]);
            },
            ..PLAIN
        },
        // Downloads still arriving, as files and as a folder.
        Case {
            name: "in_flight",
            build: |f| {
                f.files(&[
                    "movie.mp4.part",
                    "setup.dmg.crdownload",
                    "bundle.zip.download",
                    "photo.jpg.partial",
                    "agenda.pdf",
                    "menu.pdf",
                    "recipe.pdf",
                ])
                .dir("big.iso.download")
                .file("big.iso.download/chunk");
            },
            ..PLAIN
        },
        // The grace window: a recent file, a file dated in the future, and a
        // file whose modification time could not be read.
        Case {
            name: "too_recent",
            grace: Some(FIFTY_YEARS),
            tags: &[("png", "Recent"), ("pdf", "Read")],
            build: |f| {
                f.file_at("agenda.pdf", ANCIENT)
                    .file_at("menu.pdf", ANCIENT)
                    .file_at("recipe.pdf", ANCIENT)
                    .file_at("fresh.png", T0)
                    .file_at("newer.png", T0 + DAY)
                    .file_at("later.png", FUTURE)
                    .file_at("timeless.png", ANCIENT);
            },
            adjust: |scan| {
                for entry in &mut scan.entries {
                    if entry.name == "timeless.png" {
                        entry.modified = None;
                    }
                }
            },
            ..PLAIN
        },
        // Only recent files. Nothing groups, and the plan records that
        // waiting would change that (`recent_would_group`, checked below,
        // since neither output carries it).
        Case {
            name: "recent_only",
            grace: Some(FIFTY_YEARS),
            tags: &[("png", "Recent")],
            build: |f| {
                f.files(&["fresh.png", "newer.png", "latest.png"]);
            },
            ..PLAIN
        },
        // What the scan holds before planning: hidden files, links, a
        // credential-style folder, a locked file, an alias, tagged items, a
        // package, a bundle, a project folder, a folder holding a project
        // document, and an in-flight folder.
        Case {
            name: "scan_holds",
            depth: 2,
            tags: &[("pdf", "Read"), ("txt", "Read later")],
            build: |f| {
                f.files(&[".hidden.pdf", "report.pdf", "summary.pdf", "outline.pdf"])
                    .symlink("link.pdf", "report.pdf")
                    .dir("node_modules")
                    .file("node_modules/x.js")
                    .file("locked.pdf")
                    .locked("locked.pdf")
                    .alias("Shortcut.pdf")
                    .tagged("tagged.pdf", b"opaque-tag-bytes")
                    .tagged_dir("Kept")
                    .file("Kept/inside.pdf")
                    .dir("Thing.app")
                    .file("Thing.app/inside.pdf")
                    .dir("Song.band")
                    .file("Song.band/inside.pdf")
                    .dir("proj")
                    .files(&["proj/Cargo.toml", "proj/main.py"])
                    .dir("beat")
                    .files(&["beat/beat.flp", "beat/kick.wav", "beat/notes.txt"])
                    .dir("big.iso.download")
                    .file("big.iso.download/chunk");
            },
            ..PLAIN
        },
        // A group whose folder is here but locked: the group is dropped and
        // its files are counted as locked.
        Case {
            name: "locked_destination",
            build: |f| {
                f.dir("Documents").locked("Documents").files(&[
                    "agenda.pdf",
                    "menu.pdf",
                    "recipe.pdf",
                ]);
            },
            ..PLAIN
        },
        // Saved routes: a folder that is here (its file stays), a folder
        // sweep must create, no three-file minimum, and a route that takes a
        // built-in family's extension.
        Case {
            name: "saved_routes",
            depth: 2,
            maps: &[("bpy", "Blender")],
            routes: &[("bpy", "Blender"), ("pdf", "Papers")],
            build: |f| {
                f.dir("Blender").files(&[
                    "Blender/old.bpy",
                    "a.bpy",
                    "b.bpy",
                    "draft.pdf",
                    "Screenshot 2026-01-01 at 10.00.00.png",
                    "Screenshot 2026-01-01 at 10.00.01.png",
                    "Screenshot 2026-01-01 at 10.00.02.png",
                ]);
            },
            ..PLAIN
        },
        // One-shot maps: a new folder with enough files, a folder that is
        // here with fewer, and a new folder with too few. The same pairs as
        // saved routes have no minimum.
        Case {
            name: "one_shot_maps",
            depth: 2,
            maps: &[("bpy", "Blends"), ("fbx", "Models"), ("stl", "Prints")],
            routes: &[("bpy", "Blends"), ("fbx", "Models"), ("stl", "Prints")],
            tags: &[("stl", "Print")],
            build: |f| {
                f.dir("Models").files(&[
                    "a.bpy", "b.bpy", "c.bpy", "x.fbx", "y.fbx", "p.stl", "q.stl",
                ]);
            },
            ..PLAIN
        },
        // The tag scheme: a tag already on a file here, a Finder colour tag,
        // a file that already has its tag, a file whose tags cannot be read,
        // a tag nothing matches, and a file already inside its built-in
        // folder (which the tag scheme also leaves alone).
        Case {
            name: "tags",
            depth: 2,
            include_tagged: true,
            tags: &[("pdf", "Work"), ("png", "red"), ("txt", "Later")],
            build: |f| {
                let work = etude_core::tags::encode(&["Work".to_string()]).unwrap();
                let blue = etude_core::tags::encode(&["Blue\n4".to_string()]).unwrap();
                f.files(&["report.pdf", "photo.png", "picture.png", "passport.pdf"])
                    .tagged("memo.pdf", &work)
                    .tagged("shot.png", &blue)
                    .tagged("plan.pdf", b"not a property list")
                    .dir("Documents")
                    .file("Documents/old.pdf");
            },
            ..PLAIN
        },
        // The planner's own project-document holds. A real scan already
        // holds a loose document and its neighbours (the `project_document`
        // folder above), so the planner's copy of that rule never sees one.
        // This hands the held files back to the planner as ordinary entries,
        // which pins what the planner's copy does: the document is held, the
        // families it could reference are held, screenshots are exempt.
        Case {
            name: "plan_time_document",
            tags: &[("pdf", "Read"), ("png", "Seen")],
            build: |f| {
                f.files(&[
                    "scene.blend",
                    "wood.png",
                    "stone.png",
                    "metal.png",
                    "rig.py",
                    "notes.pdf",
                    "brief.pdf",
                    "credits.pdf",
                    "Screenshot 2026-01-01 at 10.00.00.png",
                    "Screenshot 2026-01-01 at 10.00.01.png",
                    "Screenshot 2026-01-01 at 10.00.02.png",
                ]);
            },
            adjust: |scan| {
                for (path, _) in std::mem::take(&mut scan.project_holds) {
                    let meta = fs::symlink_metadata(&path).unwrap();
                    let name = path.file_name().unwrap().to_string_lossy().into_owned();
                    let ext = path
                        .extension()
                        .map(|e| e.to_string_lossy().to_ascii_lowercase())
                        .unwrap_or_default();
                    scan.entries.push(etude_core::Entry {
                        path,
                        name,
                        ext,
                        size: meta.len(),
                        modified: meta.modified().ok(),
                        is_dir: false,
                        is_package: false,
                    });
                }
                scan.entries.sort_by(|a, b| a.path.cmp(&b.path));
            },
            ..PLAIN
        },
        // A bit of everything in one folder, two levels deep.
        Case {
            name: "mixed",
            depth: 2,
            grace: Some(FIFTY_YEARS),
            maps: &[("bpy", "Blends")],
            routes: &[("bpy", "Blends"), ("csv", "Sheets")],
            tags: &[("pdf", "Work"), ("bpy", "3D")],
            build: |f| {
                f.file_at("Screenshot 2026-01-01 at 10.00.00.png", ANCIENT)
                    .file_at("Screenshot 2026-01-01 at 10.00.01.png", ANCIENT)
                    .file_at("IMG_0001.JPG", ANCIENT)
                    .file_at("IMG_0002.JPG", ANCIENT + HOUR)
                    .file_at("Setup.dmg", ANCIENT)
                    .file_at("Tool.pkg", ANCIENT)
                    .file_at("Driver.dmg", ANCIENT)
                    .file_at("agenda.pdf", ANCIENT)
                    .file_at("bloodwork-summary.pdf", ANCIENT)
                    .file_at("tax_return_2023.pdf", ANCIENT)
                    .file_at("movie.mp4.part", ANCIENT)
                    .file_at("fresh.pdf", T0)
                    .file_at("a.bpy", ANCIENT)
                    .file_at("b.bpy", ANCIENT)
                    .file_at("c.bpy", ANCIENT)
                    .file_at("table.csv", ANCIENT)
                    .file_at("odd.xyz", ANCIENT)
                    .dir("Documents")
                    .file_at("Documents/old.pdf", ANCIENT)
                    .dir("proj")
                    .file_at("proj/package.json", ANCIENT);
            },
            ..PLAIN
        },
    ]
}

fn scan_case(case: &Case) -> (Fixture, ScanOutcome) {
    let mut fixture = Fixture::new(case.name);
    (case.build)(&mut fixture);
    fixture.finish();
    let config = ScanConfig {
        depth: case.depth,
        grace: case.grace,
        include_tagged: case.include_tagged,
        ..Default::default()
    };
    let mut outcome = scan::scan(&fixture.root, &config)
        .unwrap_or_else(|e| panic!("{}: scan failed: {e:?}", case.name));
    (case.adjust)(&mut outcome);
    (fixture, outcome)
}

fn owned(pairs: Pairs) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

/// Every public plan entry point, in a fixed order.
fn plans(case: &Case, scan: &ScanOutcome) -> Vec<(&'static str, Plan)> {
    let maps = plan::validate_maps(&owned(case.maps), &scan.root)
        .unwrap_or_else(|e| panic!("{}: maps: {e}", case.name));
    let routes = plan::validate_existing_maps(&owned(case.routes), &scan.root)
        .unwrap_or_else(|e| panic!("{}: routes: {e}", case.name));
    let specs = if case.tags.is_empty() {
        Vec::new()
    } else {
        plan::validate_tag_maps(&owned(case.tags))
            .unwrap_or_else(|e| panic!("{}: tags: {e}", case.name))
    };
    vec![
        ("build", plan::build(scan)),
        ("build_mapped", plan::build_mapped(scan, &maps)),
        ("build_existing", plan::build_existing(scan, &routes)),
        (
            "build_existing_with",
            plan::build_existing_with(scan, &mut GoldenInspector, &routes),
        ),
        (
            "build_with",
            plan::build_with(scan, Some(&mut GoldenInspector)),
        ),
        (
            "build_with_maps",
            plan::build_with_maps(scan, Some(&mut GoldenInspector), &maps),
        ),
        (
            "build_tags",
            plan::build_tags(scan, &specs, Some(&mut GoldenInspector)),
        ),
    ]
}

// --- normalising ------------------------------------------------------------

/// Replaces the run's temp root in a path with `$ROOT`.
struct Rooted {
    roots: Vec<Vec<u8>>,
}

impl Rooted {
    fn new(fixture: &Fixture, plan: &Plan) -> Self {
        let mut roots: Vec<Vec<u8>> = [
            plan.root.clone(),
            fixture.root.clone(),
            fixture.root.canonicalize().unwrap(),
        ]
        .iter()
        .map(|p| p.as_os_str().as_bytes().to_vec())
        .collect();
        roots.sort_by_key(|r| std::cmp::Reverse(r.len()));
        roots.dedup();
        Self { roots }
    }

    fn bytes(&self, path: &[u8]) -> Vec<u8> {
        for root in &self.roots {
            if let Some(rest) = path.strip_prefix(root.as_slice())
                && (rest.is_empty() || rest[0] == b'/')
            {
                let mut out = ROOT_TOKEN.as_bytes().to_vec();
                out.extend_from_slice(rest);
                return out;
            }
        }
        path.to_vec()
    }

    fn text(&self, json: &str) -> String {
        let mut out = json.to_string();
        for root in &self.roots {
            out = out.replace(std::str::from_utf8(root).unwrap(), ROOT_TOKEN);
        }
        out
    }

    fn path(&self, path: &Path) -> PathBuf {
        PathBuf::from(std::ffi::OsStr::from_bytes(
            &self.bytes(path.as_os_str().as_bytes()),
        ))
    }
}

/// A plan's `--json`, one value per line. Changes no value and no order.
fn indent_json(json: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let chars: Vec<char> = json.chars().collect();
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
    };
    for (i, &c) in chars.iter().enumerate() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '{' | '[' => {
                out.push(c);
                let close = if c == '{' { '}' } else { ']' };
                if chars.get(i + 1) != Some(&close) {
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            '}' | ']' => {
                let open = if c == '}' { '{' } else { '[' };
                if i > 0 && chars[i - 1] != open {
                    depth -= 1;
                    newline(&mut out, depth);
                }
                out.push(c);
            }
            ',' => {
                out.push(c);
                newline(&mut out, depth);
            }
            ':' => out.push_str(": "),
            _ => out.push(c),
        }
    }
    out.push('\n');
    out
}

/// Rewrites exported plan bytes with the temp root replaced, the commitments
/// of absolute paths recomputed for it, and the facts a test cannot pin
/// (device, inode, change time, markers above the temp folder) set to zero
/// or empty. Walks the layout `plan/codec.rs` writes; an unknown tag or
/// section fails the test rather than being copied blind.
struct Rewrite<'a> {
    src: &'a [u8],
    at: usize,
    out: Vec<u8>,
    rooted: &'a Rooted,
    commitments: HashMap<Vec<u8>, Vec<u8>>,
}

const MAGIC_LEN: usize = 11;

impl Rewrite<'_> {
    fn take(&mut self, n: usize) -> &[u8] {
        let bytes = &self.src[self.at..self.at + n];
        self.at += n;
        bytes
    }
    fn read_n(&mut self) -> u64 {
        u64::from_le_bytes(self.take(8).try_into().unwrap())
    }
    fn read_bytes(&mut self) -> Vec<u8> {
        let n = self.read_n() as usize;
        self.take(n).to_vec()
    }
    fn put_n(&mut self, v: u64) {
        self.out.extend(v.to_le_bytes());
    }
    fn put_bytes(&mut self, v: &[u8]) {
        self.put_n(v.len() as u64);
        self.out.extend(v);
    }
    fn byte(&mut self) -> u8 {
        let b = self.take(1)[0];
        self.out.push(b);
        b
    }
    fn n(&mut self) -> u64 {
        let v = self.read_n();
        self.put_n(v);
        v
    }
    fn text(&mut self) {
        let v = self.read_bytes();
        self.put_bytes(&v);
    }
    fn path(&mut self) {
        let v = self.read_bytes();
        let v = self.rooted.bytes(&v);
        self.put_bytes(&v);
    }
    fn commitment(&mut self) {
        let v = self.read_bytes();
        let v = self.commitments.get(&v).cloned().unwrap_or(v);
        self.put_bytes(&v);
    }
    fn identity(&mut self) {
        let _device = self.read_n();
        self.put_n(0);
        let _inode = self.read_n();
        self.put_n(0);
        self.n(); // size
        self.n(); // mtime seconds
        self.n(); // mtime nanoseconds
        let _ctime = (self.read_n(), self.read_n());
        self.put_n(0);
        self.put_n(0);
        self.n(); // mode
        self.byte(); // kind
    }
    fn entries(&mut self) {
        let count = self.n();
        for _ in 0..count {
            self.text(); // a relative path's commitment
            self.identity();
        }
    }

    fn run(mut self) -> Vec<u8> {
        let magic = self.take(MAGIC_LEN).to_vec();
        self.out.extend(magic);
        for _ in 0..4 {
            self.text(); // the binding context
        }
        self.path(); // plan root
        let groups = self.n();
        for _ in 0..groups {
            self.text(); // name
            match self.byte() {
                0 | 2 => {}
                1 => {
                    self.n();
                }
                3 => {
                    let exts = self.n();
                    for _ in 0..exts {
                        self.text();
                    }
                }
                4 | 8 => {
                    self.text();
                    self.byte();
                }
                5 | 6 => {
                    self.n();
                }
                7 => {
                    self.text();
                    self.byte();
                    self.text();
                }
                tag => panic!("unknown signal tag {tag}; teach this rewriter the new layout"),
            }
            let members = self.n();
            for _ in 0..members {
                self.path();
            }
            self.byte(); // accepted
        }
        let untouched = self.n();
        for _ in 0..untouched {
            self.commitment();
            match self.byte() {
                0 => {
                    self.byte();
                }
                1..=4 | 6..=8 => {}
                5 => self.commitment(),
                tag => panic!("unknown hold tag {tag}; teach this rewriter the new layout"),
            }
        }
        for _ in 0..8 {
            self.n(); // counts
        }
        self.byte(); // root_is_synced
        self.byte(); // allow_sync
        self.path(); // snapshot root
        self.n(); // depth
        self.byte(); // whole_units
        self.identity(); // root
        // Markers above the temp folder belong to this machine, not the
        // fixture. Read them and write none.
        let ancestors = self.read_n();
        for _ in 0..ancestors {
            self.read_bytes();
            self.take(8 * 8 + 1);
        }
        self.put_n(0);
        self.entries();
        while self.at < self.src.len() {
            match self.byte() {
                1 => {
                    let count = self.n();
                    for _ in 0..count {
                        self.path();
                    }
                }
                2 => {
                    self.byte();
                    self.n();
                }
                3 => {
                    self.byte();
                }
                4 => {
                    self.n();
                }
                5 => {
                    self.n();
                    self.n();
                }
                tag => panic!("unknown trailing section {tag}; teach this rewriter"),
            }
        }
        self.out
    }
}

fn commitment_bytes(path: &Path) -> Vec<u8> {
    TreeSnapshot::path_commitment(path)
        .as_os_str()
        .as_bytes()
        .to_vec()
}

/// The exported bytes of `plan`, normalised. Exports through the public
/// path (`BoundPlan::export`), reads the file back and removes it.
fn exported(fixture: &Fixture, mode: &str, plan: &Plan, rooted: &Rooted) -> Vec<u8> {
    let mut commitments = HashMap::new();
    for (path, reason) in &plan.untouched {
        commitments.insert(commitment_bytes(path), commitment_bytes(&rooted.path(path)));
        if let Untouched::NearProjectDocument(marker) = reason {
            let marker = Path::new(marker);
            commitments.insert(
                commitment_bytes(marker),
                commitment_bytes(&rooted.path(marker)),
            );
        }
    }
    let bound = BoundPlan::from_plan(plan.clone(), context())
        .unwrap_or_else(|e| panic!("{mode}: the plan does not bind: {e}"));
    let file = fixture.base.join(format!("{mode}.plan"));
    bound
        .export(&file)
        .unwrap_or_else(|e| panic!("{mode}: the plan does not export: {e}"));
    let mut bytes = fs::read(&file).unwrap();
    fs::remove_file(&file).unwrap();
    // The digest at the end covers the raw bytes, which the rewrite changes.
    bytes.truncate(bytes.len() - 64);
    Rewrite {
        src: &bytes,
        at: 0,
        out: Vec::new(),
        rooted,
        commitments,
    }
    .run()
}

// --- comparing --------------------------------------------------------------

/// The first difference between two texts, with a few lines around it.
fn text_diff(want: &str, got: &str) -> String {
    let want: Vec<&str> = want.lines().collect();
    let got: Vec<&str> = got.lines().collect();
    let first = want
        .iter()
        .zip(&got)
        .position(|(a, b)| a != b)
        .unwrap_or(want.len().min(got.len()));
    let from = first.saturating_sub(3);
    let mut out = format!("first difference at line {}\n", first + 1);
    for line in &want[from..first] {
        out.push_str(&format!("    {line}\n"));
    }
    for line in want.iter().skip(first).take(6) {
        out.push_str(&format!("  - {line}\n"));
    }
    for line in got.iter().skip(first).take(6) {
        out.push_str(&format!("  + {line}\n"));
    }
    out
}

fn bytes_diff(want: &[u8], got: &[u8]) -> String {
    let first = want
        .iter()
        .zip(got)
        .position(|(a, b)| a != b)
        .unwrap_or(want.len().min(got.len()));
    let hex = |b: &[u8]| {
        let end = (first + 24).min(b.len());
        let start = first.min(end);
        b[start..end]
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    format!(
        "first difference at byte {first} (want {} bytes, got {})\n  - {}\n  + {}\n",
        want.len(),
        got.len(),
        hex(want),
        hex(got)
    )
}

#[test]
fn plans_match_their_golden_files() {
    let bless = std::env::var_os(BLESS).is_some_and(|v| v == "1");
    let dir = golden_dir();
    if bless {
        fs::create_dir_all(&dir).unwrap();
    }
    let cases = cases();
    assert!(
        cases.len() >= 12,
        "the golden set needs at least 12 folders"
    );

    let mut written = std::collections::BTreeSet::new();
    let mut mismatched = Vec::new();
    let mut first_diff = None;
    for case in &cases {
        let (fixture, scan) = scan_case(case);
        for (mode, plan) in plans(case, &scan) {
            let rooted = Rooted::new(&fixture, &plan);
            let json = indent_json(&rooted.text(&plan.to_json()));
            let bin = exported(&fixture, &format!("{}__{mode}", case.name), &plan, &rooted);
            let temp = std::env::temp_dir();
            assert!(
                !json.contains(temp.to_str().unwrap()),
                "{}__{mode}: a temp path survived normalising",
                case.name
            );
            for (ext, got) in [("json", json.into_bytes()), ("bin", bin)] {
                let name = format!("{}__{mode}.{ext}", case.name);
                let path = dir.join(&name);
                written.insert(name.clone());
                if bless {
                    fs::write(&path, &got).unwrap();
                    continue;
                }
                let want = fs::read(&path).unwrap_or_default();
                if want != got {
                    if first_diff.is_none() {
                        first_diff = Some(if ext == "json" {
                            text_diff(
                                &String::from_utf8_lossy(&want),
                                &String::from_utf8_lossy(&got),
                            )
                        } else {
                            bytes_diff(&want, &got)
                        });
                        first_diff = first_diff.map(|d| format!("{name}: {d}"));
                    }
                    mismatched.push(name);
                }
            }
        }
    }

    // A golden file no fixture writes any more is a test that silently
    // stopped running.
    let on_disk: Vec<String> = fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let orphans: Vec<&String> = on_disk.iter().filter(|n| !written.contains(*n)).collect();
    if bless {
        for name in orphans {
            fs::remove_file(dir.join(name)).unwrap();
        }
        return;
    }
    assert!(
        orphans.is_empty(),
        "golden files no fixture produces: {orphans:?}"
    );
    assert!(
        mismatched.is_empty(),
        "{} golden file(s) differ: {mismatched:?}\n{}\nIf the change is meant, run {BLESS}=1 \
         cargo test -p etude-core --test plan_golden and name each changed file in a \
         Golden-Change: trailer.",
        mismatched.len(),
        first_diff.unwrap_or_default()
    );
}

/// `recent_would_group` is decided while planning but is in neither golden
/// file, so it is pinned here: with only recent files nothing groups, and
/// ignoring the window would group them.
#[test]
fn recent_files_alone_would_group_once_the_window_passes() {
    let cases = cases();
    for case in &cases {
        let (_fixture, scan) = scan_case(case);
        for (mode, plan) in plans(case, &scan) {
            let expected = case.name == "recent_only";
            assert_eq!(
                plan.recent_would_group, expected,
                "{}__{mode}: recent_would_group",
                case.name
            );
        }
    }
}
