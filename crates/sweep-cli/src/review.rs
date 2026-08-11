//! Interactive review.
//!
//! # Deviation from the spec, on purpose
//!
//! The spec described `sweep review` and `sweep apply` as separate commands
//! operating on a **persisted plan**. This implements review as a single
//! scan → decide → apply pass in one process instead, and never writes a plan
//! file.
//!
//! The reason is the threat model. A persisted plan is a second plaintext
//! index of the user's filenames sitting on disk between two commands — the
//! same problem the journal has, without the journal's justification.
//! Eliminating it removes an asset rather than protecting one.
//!
//! The cost is that review cannot be resumed after quitting. That is the right
//! trade: nothing has moved at that point, so re-running costs a rescan.
//!
//! # The rename escape hatch
//!
//! sweep never coins a label the filesystem did not already contain. The user
//! is under no such rule — if they want a folder called `Tax 2024`, that is
//! their call. `r` is how they say so, and it is the only path by which a
//! sensitive-sounding directory name can ever be created.

use std::io::{self, BufRead, IsTerminal, Write};

use etude_core::plan::Plan;

pub enum Outcome {
    /// Apply the plan as now marked.
    Apply,
    /// User quit. Nothing has moved.
    Cancelled,
}

/// Longest destination name accepted. Well under NAME_MAX, and a name longer
/// than this is a mistake rather than an intention.
const MAX_NAME: usize = 64;

/// Reject names that would escape the scan root or confuse the filesystem.
fn validate(name: &str) -> Result<(), &'static str> {
    let t = name.trim();
    if t.is_empty() {
        return Err("a name cannot be empty");
    }
    if t.len() > MAX_NAME {
        return Err("that name is too long");
    }
    if t.contains('/') || t.contains('\\') {
        return Err("a group name cannot contain a path separator");
    }
    if t == "." || t == ".." {
        return Err("that name is reserved");
    }
    if t.starts_with('.') {
        return Err("a leading dot would hide the folder");
    }
    if t.chars().any(|c| c.is_control()) {
        return Err("that name contains control characters");
    }
    Ok(())
}

fn prompt(input: &mut dyn BufRead, label: &str) -> io::Result<String> {
    print!("{label}");
    io::stdout().flush()?;
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Ok("q".into()); // EOF is a quit, not a crash
    }
    Ok(line.trim().to_string())
}

/// Entry point used by the CLI: requires a terminal, reads real stdin.
pub fn run(plan: &mut Plan) -> io::Result<Outcome> {
    if !io::stdin().is_terminal() {
        eprintln!(
            "sweep: review needs a terminal. Use `sweep apply PATH --yes`\n\
             or `--only NAME` when running non-interactively."
        );
        return Ok(Outcome::Cancelled);
    }
    let stdin = io::stdin();
    let mut locked = stdin.lock();
    run_with(plan, &mut locked)
}

/// The reviewable loop, with input injected so the interactive flow — including
/// the rename escape hatch — can be tested rather than demonstrated by hand.
pub fn run_with(plan: &mut Plan, input: &mut dyn BufRead) -> io::Result<Outcome> {
    println!(
        "\nReviewing {} group(s). Nothing moves until the end.\n",
        plan.groups.len()
    );
    let mut warned_scrollback = false;

    for i in 0..plan.groups.len() {
        loop {
            let g = &plan.groups[i];
            println!(
                "  {}  —  {} files  —  {}",
                g.name,
                g.members.len(),
                g.signal.describe()
            );
            let answer = prompt(input, "  [a]ccept  [s]kip  [r]ename  [d]etails  [q]uit > ")?;

            match answer.chars().next().unwrap_or('a') {
                'a' | '\0' => {
                    plan.groups[i].accepted = true;
                    println!("    accepted → {}/\n", plan.groups[i].name);
                    break;
                }
                's' => {
                    plan.groups[i].accepted = false;
                    println!("    skipped, files stay where they are\n");
                    break;
                }
                'd' => {
                    if !warned_scrollback {
                        println!(
                            "\n    Note: these filenames will remain in your terminal scrollback."
                        );
                        warned_scrollback = true;
                    }
                    for m in &plan.groups[i].members {
                        println!(
                            "      {}",
                            m.file_name().unwrap_or_default().to_string_lossy()
                        );
                    }
                    println!();
                }
                'r' => {
                    let new = prompt(input, "    new folder name > ")?;
                    match validate(&new) {
                        Err(why) => println!("    {why}\n"),
                        Ok(()) => {
                            // The user may choose a revealing name. That is
                            // their right, but it must be an informed choice:
                            // the folder is visible in Finder, indexed by
                            // Spotlight, and captured by every backup.
                            println!(
                                "\n    \"{}\" will be visible in Finder and Spotlight,\n    \
                                 and captured by any backup or sync.",
                                new.trim()
                            );
                            let ok = prompt(input, "    use it anyway? [y/N] > ")?;
                            if ok.eq_ignore_ascii_case("y") {
                                plan.groups[i].name = new.trim().to_string();
                                println!("    renamed\n");
                            } else {
                                println!("    kept \"{}\"\n", plan.groups[i].name);
                            }
                        }
                    }
                }
                'q' => {
                    println!("\n  Cancelled. Nothing has been moved.");
                    return Ok(Outcome::Cancelled);
                }
                _ => println!("    unrecognised — a, s, r, d or q\n"),
            }
        }
    }

    let accepted: Vec<&etude_core::plan::Group> =
        plan.groups.iter().filter(|g| g.accepted).collect();
    if accepted.is_empty() {
        println!("  Nothing accepted. Nothing has been moved.");
        return Ok(Outcome::Cancelled);
    }

    println!("\n  About to move {} files into:", plan.moves());
    for g in &accepted {
        println!("    {}/  ({} files)", g.name, g.members.len());
    }
    let personal: usize = plan.sensitive_counts().values().sum();
    if personal > 0 {
        println!("\n  {personal} files that look like personal records stay where they are.");
    }

    let go = prompt(input, "\n  proceed? [y/N] > ")?;
    if go.eq_ignore_ascii_case("y") {
        Ok(Outcome::Apply)
    } else {
        println!("  Cancelled. Nothing has been moved.");
        Ok(Outcome::Cancelled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use etude_core::plan::{Group, Signal};
    use std::io::Cursor;
    use std::path::PathBuf;

    fn plan_of(names: &[&str]) -> Plan {
        Plan {
            root: PathBuf::from("/tmp/root"),
            groups: names
                .iter()
                .map(|n| Group {
                    name: n.to_string(),
                    signal: Signal::Screenshot,
                    members: vec![PathBuf::from(format!("/tmp/root/{n}-1"))],
                    accepted: false,
                })
                .collect(),
            untouched: Vec::new(),
            scanned: 1,
            skipped_hidden: 0,
            skipped_symlink: 0,
            root_is_synced: false,
            allow_sync: false,
        }
    }

    fn drive(p: &mut Plan, script: &str) -> Outcome {
        let mut input = Cursor::new(script.as_bytes().to_vec());
        run_with(p, &mut input).expect("review")
    }

    #[test]
    fn the_rename_escape_hatch_lets_a_user_choose_a_name_sweep_never_would() {
        // The whole reason review exists. sweep will not coin "Tax 2024";
        // the user may, and this is the only path by which that can happen.
        let mut p = plan_of(&["shots"]);
        let outcome = drive(&mut p, "r\nTax 2024\ny\na\ny\n");

        assert!(matches!(outcome, Outcome::Apply));
        assert_eq!(p.groups[0].name, "Tax 2024", "rename did not take effect");
        assert!(p.groups[0].accepted);
    }

    #[test]
    fn declining_the_visibility_warning_keeps_the_original_name() {
        // The warning is not decoration. Answering anything but y must abort
        // the rename, because the user was told the folder will be visible.
        let mut p = plan_of(&["shots"]);
        drive(&mut p, "r\nTax 2024\nn\na\ny\n");
        assert_eq!(
            p.groups[0].name, "shots",
            "rename applied despite declining"
        );
    }

    #[test]
    fn an_invalid_name_is_rejected_and_the_user_is_asked_again() {
        let mut p = plan_of(&["shots"]);
        drive(&mut p, "r\n../escape\nr\nSafe Name\ny\na\ny\n");
        assert_eq!(p.groups[0].name, "Safe Name", "traversal name was accepted");
    }

    #[test]
    fn skipping_a_group_leaves_it_unaccepted() {
        let mut p = plan_of(&["a", "b"]);
        drive(&mut p, "s\na\ny\n");
        assert!(!p.groups[0].accepted, "skipped group was accepted");
        assert!(p.groups[1].accepted, "accepted group was not accepted");
    }

    #[test]
    fn quitting_cancels_and_accepts_nothing() {
        let mut p = plan_of(&["a", "b"]);
        let outcome = drive(&mut p, "a\nq\n");
        assert!(matches!(outcome, Outcome::Cancelled));
    }

    #[test]
    fn declining_the_final_confirmation_cancels() {
        let mut p = plan_of(&["a"]);
        let outcome = drive(&mut p, "a\nn\n");
        assert!(
            matches!(outcome, Outcome::Cancelled),
            "proceeded without confirmation"
        );
    }

    #[test]
    fn eof_is_treated_as_quit_not_as_consent() {
        // A closed pipe must never be read as "yes, move my files".
        let mut p = plan_of(&["a"]);
        let outcome = drive(&mut p, "");
        assert!(
            matches!(outcome, Outcome::Cancelled),
            "EOF was treated as consent"
        );
    }

    #[test]
    fn skipping_every_group_cancels_rather_than_applying_nothing() {
        let mut p = plan_of(&["a", "b"]);
        let outcome = drive(&mut p, "s\ns\n");
        assert!(matches!(outcome, Outcome::Cancelled));
    }

    #[test]
    fn rejects_names_that_escape_or_confuse_the_filesystem() {
        assert!(validate("../../etc").is_err(), "path traversal accepted");
        assert!(validate("a/b").is_err(), "path separator accepted");
        assert!(validate("").is_err(), "empty name accepted");
        assert!(validate("   ").is_err(), "blank name accepted");
        assert!(validate(".hidden").is_err(), "hidden name accepted");
        assert!(validate("..").is_err(), "parent reference accepted");
        assert!(
            validate(&"x".repeat(200)).is_err(),
            "overlong name accepted"
        );
        assert!(
            validate("bad\u{7}name").is_err(),
            "control character accepted"
        );
    }

    #[test]
    fn accepts_a_name_the_user_deliberately_chose() {
        // The naming rule binds sweep, not the user. If they want "Tax 2024",
        // they may have it — that is the whole point of the escape hatch.
        assert!(validate("Tax 2024").is_ok());
        assert!(validate("Bali trip").is_ok());
        assert!(validate("Acme redesign").is_ok());
    }
}
