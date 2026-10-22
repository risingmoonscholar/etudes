//! What `--scheme tags` says: the plan, the apply result and the undo result.
//!
//! A tag plan adds Finder tags and moves nothing. The wording keeps three
//! facts in view every time: tags already on a file stay, nothing is moved,
//! and a tag is called new only when no file in the scanned folder carries it.

use std::process::ExitCode;

use etude_core::apply::{ApplyReport, TagRefusal, UndoReport};
use etude_core::json as j;
use etude_core::plan::{Plan, TAG_LIST_NOTE, TAG_NEW_RULE};

fn files(n: usize) -> &'static str {
    if n == 1 { "file" } else { "files" }
}

/// The scan line every plan opens with.
fn banner(p: &Plan, read_contents: bool) {
    let basis = if read_contents {
        "filesystem and Finder organization metadata, and the contents of some text files"
    } else {
        "filesystem and Finder organization metadata  ·  no contents read to decide"
    };
    println!(
        "\nScanned {} items  ·  {basis}  ·  sweep {}\n",
        p.scanned,
        env!("CARGO_PKG_VERSION")
    );
}

/// Lines about files the plan will not tag, each with its own reason.
fn left_alone(p: &Plan) {
    let present = p.tag_already_present();
    if present == 1 {
        println!("  1 file already carries its tag and is left as it is");
    } else if present > 1 {
        println!("  {present} files already carry their tag and are left as they are");
    }
    let unreadable = p.tag_state_unreadable();
    if unreadable == 1 {
        println!("  1 file's tags could not be read; nothing will be added to it");
    } else if unreadable > 1 {
        println!("  {unreadable} files' tags could not be read; nothing will be added to them");
    }
    let unmapped = p.no_clear_group();
    if unmapped == 1 {
        println!("  1 file has an extension no --map names and will not be tagged");
    } else if unmapped > 1 {
        println!("  {unmapped} files have an extension no --map names and will not be tagged");
    }
}

fn promise() {
    println!(
        "Applying adds the tags above to those files. Tags a file already has stay, in their order."
    );
    println!(
        "Nothing is moved, renamed, created or deleted. Sweep reads each file's tag list, never its contents."
    );
}

pub fn render(p: &Plan, quiet: bool, explain: bool, read_contents: bool) {
    banner(p, read_contents);
    let rows = p.tag_summary();
    let width = rows
        .iter()
        .map(|row| row.name.chars().count())
        .max()
        .unwrap_or(8)
        .max(8);
    println!("  Tags to add");
    for row in &rows {
        let extensions: Vec<String> = row.extensions.iter().map(|ext| format!(".{ext}")).collect();
        println!(
            "  {:<width$}  {:>3} {:<5}  {}{}",
            row.name,
            row.files,
            files(row.files),
            extensions.join(", "),
            if row.new { "   NEW TAG" } else { "" },
            width = width
        );
    }
    let new: Vec<&str> = rows
        .iter()
        .filter(|row| row.new)
        .map(|row| row.name.as_str())
        .collect();
    println!();
    match new.len() {
        0 => println!("  Every tag above is already on files in this folder."),
        1 => println!("  1 tag is new: {}", new[0]),
        n => println!("  {n} tags are new: {}", new.join(", ")),
    }
    println!("  {TAG_NEW_RULE}");
    println!("  {TAG_LIST_NOTE}");
    left_alone(p);
    super::print_left_alone_notes(p, explain);
    if p.skipped_hidden + p.skipped_symlink > 0 {
        println!(
            "\n  skipped {} hidden {} and {} {}",
            p.skipped_hidden,
            if p.skipped_hidden == 1 {
                "item"
            } else {
                "items"
            },
            p.skipped_symlink,
            if p.skipped_symlink == 1 {
                "symlink"
            } else {
                "symlinks"
            }
        );
    }
    super::print_refused_by_policy_note(p);
    super::print_unreadable_warning(p);
    if p.root_is_synced {
        println!("\n  warning: this folder is inside a cloud-synced tree");
    }
    if explain && !quiet {
        println!("\n--- signal trace ---");
        for group in &p.groups {
            println!("\n{}: {}", group.name, group.signal.describe());
            for member in &group.members {
                println!("    {}", member.display());
            }
        }
    }
    println!("\nNothing has been tagged or moved.  Nothing left this machine.");
    if !quiet {
        println!("Note: this listing is in your terminal scrollback.");
    }
    println!(
        "Apply: save this plan with --export-plan FILE, then run\n       sweep apply --plan FILE --plan-digest SHA --yes"
    );
    promise();
    println!("Undo puts each file's previous tags back: sweep undo");
}

/// A tag plan that tags no file.
pub fn nothing_to_tag(p: &Plan) {
    println!(
        "\nScanned {} items  ·  filesystem and Finder organization metadata  ·  no contents read to decide  ·  sweep {}",
        p.scanned,
        env!("CARGO_PKG_VERSION")
    );
    super::print_refused_by_policy_note(p);
    super::print_unreadable_warning(p);
    println!();
    left_alone(p);
    println!("\nNo file needs a tag.");
    println!("\nNothing has been tagged or moved.  Nothing left this machine.");
}

/// Report a tag apply that ran to the end, and pick its exit.
pub fn finish_apply(r: &ApplyReport, binding: &str) -> ExitCode {
    use etude_cli_support::envelope;
    envelope::effect("items_tagged", r.tagged);
    envelope::detail(j::obj(&[
        ("tagged", j::num(r.tagged)),
        ("moved", j::num(0)),
        ("tag_skipped_unreadable", j::num(r.tag_skipped_unreadable)),
        ("tag_skipped_present", j::num(r.tag_skipped_present)),
        ("tag_skipped_changed", j::num(r.tag_skipped_changed)),
        ("binding", binding.to_string()),
        (
            "journal",
            r.journal_path
                .as_deref()
                .map(j::path)
                .unwrap_or_else(|| "null".into()),
        ),
    ]));
    println!(
        "\nTagged {} {}. Tags the files already had were kept. Nothing was moved.",
        r.tagged,
        files(r.tagged)
    );
    if r.tag_skipped_present > 0 {
        println!(
            "{} {} already had the tag and {} left as found.",
            r.tag_skipped_present,
            files(r.tag_skipped_present),
            if r.tag_skipped_present == 1 {
                "was"
            } else {
                "were"
            }
        );
    }
    if r.tag_skipped_unreadable > 0 {
        println!(
            "{} {} had tags that could not be read (or could not hold one more); nothing was written to {}.",
            r.tag_skipped_unreadable,
            files(r.tag_skipped_unreadable),
            if r.tag_skipped_unreadable == 1 {
                "it"
            } else {
                "them"
            }
        );
    }
    if r.tag_skipped_changed > 0 {
        println!(
            "{} {} had their tags change after the plan was made and {} left as found.",
            r.tag_skipped_changed,
            files(r.tag_skipped_changed),
            if r.tag_skipped_changed == 1 {
                "was"
            } else {
                "were"
            }
        );
    }
    if r.tagged > 0 {
        match &r.journal_path {
            Some(path) => {
                println!("Undo with: sweep undo");
                println!("Encrypted journal: {}", path.display());
            }
            None => println!("No journal was written. This cannot be undone."),
        }
    }
    println!("\nNothing left this machine.");
    if r.tagged == 0 {
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

pub fn undo_detail(r: &UndoReport) {
    etude_cli_support::envelope::detail(j::obj(&[
        ("restored", j::num(0)),
        ("tags_restored", j::num(r.tags_restored)),
        (
            "directories_left_for_journals",
            j::num(r.directories_left_for_journals),
        ),
        ("skipped_changed", j::num(r.skipped_changed.len())),
        (
            "tags_refused_changed",
            j::num(
                r.tags_refused
                    .iter()
                    .filter(|(_, why)| *why == TagRefusal::Changed)
                    .count(),
            ),
        ),
        (
            "tags_refused_unreadable",
            j::num(
                r.tags_refused
                    .iter()
                    .filter(|(_, why)| *why == TagRefusal::Unreadable)
                    .count(),
            ),
        ),
        ("skipped_missing", j::num(r.skipped_missing.len())),
    ]));
}

pub fn print_undo_headline(r: &UndoReport) {
    println!(
        "\nRestored the previous tags on {} {}. Nothing was moved.",
        r.tags_restored,
        files(r.tags_restored)
    );
}

pub fn print_undo_refusals(r: &UndoReport) {
    let changed: Vec<_> = r
        .tags_refused
        .iter()
        .filter(|(_, why)| *why == TagRefusal::Changed)
        .collect();
    if !changed.is_empty() {
        let (noun, had, was) = if changed.len() == 1 {
            ("file", "had its", "was")
        } else {
            ("files", "had their", "were")
        };
        println!(
            "  {} {noun} {had} tags changed since apply and {was} left alone (restoring would overwrite that change):",
            changed.len(),
        );
        for (path, _) in changed {
            println!("    {}", etude_core::redact::path(path));
        }
    }
    let unreadable: Vec<_> = r
        .tags_refused
        .iter()
        .filter(|(_, why)| *why == TagRefusal::Unreadable)
        .collect();
    if !unreadable.is_empty() {
        println!(
            "  {} {} had tags that could not be read and {} left alone:",
            unreadable.len(),
            files(unreadable.len()),
            if unreadable.len() == 1 { "was" } else { "were" }
        );
        for (path, _) in unreadable {
            println!("    {}", etude_core::redact::path(path));
        }
    }
}
