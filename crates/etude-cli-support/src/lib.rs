//! Small presentation helpers shared by the command-line tools.

use std::io::{self, IsTerminal, Write};

const MIN_ITEMS_FOR_PROGRESS: usize = 100;
const REPORTS_PER_OPERATION: usize = 10;

/// Emits bounded item-count progress without changing command output or its
/// machine-readable stdout. TTY output is a single updating line; redirected
/// output receives at most ten milestone lines. Small operations stay quiet.
pub struct ProgressReporter<W: Write> {
    writer: W,
    label: &'static str,
    total: usize,
    interval: usize,
    next_report: usize,
    terminal: bool,
    line_open: bool,
}

impl ProgressReporter<io::Stderr> {
    pub fn stderr(label: &'static str, total: usize) -> Self {
        let stderr = io::stderr();
        let terminal = stderr.is_terminal();
        Self::with_writer(label, total, stderr, terminal)
    }
}

impl<W: Write> ProgressReporter<W> {
    pub fn with_writer(label: &'static str, total: usize, writer: W, terminal: bool) -> Self {
        let interval = total.div_ceil(REPORTS_PER_OPERATION).max(1);
        Self {
            writer,
            label,
            total,
            interval,
            next_report: interval,
            terminal,
            line_open: false,
        }
    }

    pub fn update(&mut self, completed: usize, total: usize) {
        if self.total < MIN_ITEMS_FOR_PROGRESS
            || total != self.total
            || completed == 0
            || completed > total
            || (completed < self.next_report && completed != total)
        {
            return;
        }

        if self.terminal {
            let _ = write!(self.writer, "\r{}: {completed}/{total} items", self.label);
            let _ = self.writer.flush();
            self.line_open = true;
        } else {
            let _ = writeln!(self.writer, "{}: {completed}/{total} items", self.label);
        }

        self.next_report = self.next_report.saturating_add(self.interval);
    }
}

impl<W: Write> Drop for ProgressReporter<W> {
    fn drop(&mut self) {
        if self.line_open {
            let _ = writeln!(self.writer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProgressReporter;

    #[test]
    fn small_operations_stay_quiet_and_large_operations_emit_bounded_milestones() {
        let mut output = Vec::new();
        {
            let mut small = ProgressReporter::with_writer("apply", 99, &mut output, false);
            small.update(50, 99);
            small.update(99, 99);
        }
        assert!(output.is_empty());

        {
            let mut large = ProgressReporter::with_writer("apply", 1_000, &mut output, false);
            for completed in 1..=1_000 {
                large.update(completed, 1_000);
            }
        }
        let rendered = String::from_utf8(output).expect("progress is UTF-8");
        let lines: Vec<_> = rendered.lines().collect();
        assert_eq!(lines.len(), 10, "large work is bounded to ten updates");
        assert_eq!(lines.first(), Some(&"apply: 100/1000 items"));
        assert_eq!(lines.last(), Some(&"apply: 1000/1000 items"));
    }

    #[test]
    fn terminal_progress_updates_one_line_and_terminates_it() {
        let mut output = Vec::new();
        {
            let mut progress = ProgressReporter::with_writer("undo", 100, &mut output, true);
            progress.update(10, 100);
            progress.update(100, 100);
        }
        let rendered = String::from_utf8(output).expect("progress is UTF-8");
        assert_eq!(rendered.matches('\r').count(), 2);
        assert!(rendered.ends_with('\n'));
        assert!(rendered.contains("undo: 100/100 items"));
    }
}
