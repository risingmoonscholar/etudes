//! The one progress layer every tool shares: the `--progress-json` flag, the
//! schema, the writer, and the human milestones. A tool supplies counts it
//! observed and nothing else; see docs/progress.md for what each count means.
use etude_core::{
    apply::{ProgressPhase, StructuredProgress},
    json as j,
};
use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

pub const FLAG: &str = "--progress-json";
pub const SCHEMA_VERSION: u32 = 1;
pub const FIELDS: &[&str] = &[
    "schema_version",
    "event",
    "tool_version",
    "operation_id",
    "operation",
    "phase",
    "planned",
    "completed",
    "journalled",
];
pub const MAX_EVENTS: usize = 12;
const MAX_ADVANCES: usize = 10;

/// Writes one NDJSON record per call, and caps them. Every record is formatted
/// first and handed to the writer whole, so a kill between records cannot tear
/// one and an extractor sharing stderr cannot land inside one.
pub struct JsonProgress<W: Write> {
    writer: W,
    version: String,
    operation_id: String,
    operation: &'static str,
    next: usize,
    interval: usize,
    advances_left: usize,
}
impl JsonProgress<io::Stderr> {
    pub fn stderr(version: &str, operation_id: &str, operation: &'static str) -> Self {
        Self::new(io::stderr(), version, operation_id, operation)
    }
}
impl<W: Write> JsonProgress<W> {
    pub fn new(writer: W, version: &str, operation_id: &str, operation: &'static str) -> Self {
        Self {
            writer,
            version: version.into(),
            operation_id: operation_id.into(),
            operation,
            next: 1,
            interval: 1,
            advances_left: MAX_ADVANCES,
        }
    }

    /// Work has started and the count is not known yet: `planning` with
    /// `planned` zero. It takes one advancing record out of the budget, so a
    /// run still writes at most `MAX_EVENTS`.
    pub fn indeterminate(&mut self) {
        self.advances_left = self.advances_left.saturating_sub(1);
        self.write(ProgressPhase::Planning, 0, 0, 0);
    }

    pub fn update(&mut self, event: StructuredProgress) {
        match event.phase {
            ProgressPhase::Planning => {
                if event.planned == 0 {
                    return;
                }
                self.interval = event.planned.div_ceil(self.advances_left.max(1)).max(1);
                self.next = self.interval;
            }
            ProgressPhase::Advancing => {
                if event.completed < self.next || self.advances_left == 0 {
                    return;
                }
                self.advances_left -= 1;
                self.next = event.completed.saturating_add(self.interval);
            }
            ProgressPhase::Done | ProgressPhase::Error => {}
        }
        self.write(
            event.phase,
            event.planned,
            event.completed,
            event.journalled,
        );
    }

    fn write(&mut self, phase: ProgressPhase, planned: usize, completed: usize, journalled: usize) {
        let phase = match phase {
            ProgressPhase::Planning => "planning",
            ProgressPhase::Advancing => "advancing",
            ProgressPhase::Done => "done",
            ProgressPhase::Error => "error",
        };
        let mut record = j::obj(&[
            ("schema_version", j::num(SCHEMA_VERSION)),
            ("event", j::str("progress")),
            ("tool_version", j::str(&self.version)),
            ("operation_id", j::str(&self.operation_id)),
            ("operation", j::str(self.operation)),
            ("phase", j::str(phase)),
            ("planned", j::num(planned)),
            ("completed", j::num(completed)),
            ("journalled", j::num(journalled)),
        ]);
        record.push('\n');
        // An output failure never changes the operation it describes.
        let _ = self.writer.write_all(record.as_bytes());
        let _ = self.writer.flush();
    }
}

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
    last_report: usize,
    terminal: bool,
    line_open: bool,
}

impl<W: Write> ProgressReporter<W> {
    pub fn with_writer(label: &'static str, total: usize, writer: W, terminal: bool) -> Self {
        let mut reporter = Self {
            writer,
            label,
            total: 0,
            interval: 1,
            next_report: 1,
            last_report: 0,
            terminal,
            line_open: false,
        };
        reporter.set_total(total);
        reporter
    }

    fn set_total(&mut self, total: usize) {
        self.total = total;
        self.interval = total.div_ceil(REPORTS_PER_OPERATION).max(1);
        self.next_report = self.interval;
    }

    pub fn update(&mut self, completed: usize, total: usize) {
        if self.total < MIN_ITEMS_FOR_PROGRESS
            || total != self.total
            || completed <= self.last_report
            || completed > total
            || (completed < self.next_report && completed != total)
        {
            return;
        }

        let noun = if total == 1 { "item" } else { "items" };
        if self.terminal {
            let _ = write!(self.writer, "\r{}: {completed}/{total} {noun}", self.label);
            let _ = self.writer.flush();
            self.line_open = true;
        } else {
            let _ = writeln!(self.writer, "{}: {completed}/{total} {noun}", self.label);
        }

        self.last_report = completed;
        while self.next_report <= completed {
            self.next_report = self.next_report.saturating_add(self.interval);
        }
    }

    /// Close an open TTY line, so the operation's own output starts clean.
    fn settle(&mut self) {
        if self.line_open {
            let _ = writeln!(self.writer);
            self.line_open = false;
        }
    }
}

impl<W: Write> Drop for ProgressReporter<W> {
    fn drop(&mut self) {
        self.settle();
    }
}

enum Output<W: Write> {
    Human(ProgressReporter<W>),
    Json(JsonProgress<W>),
}

/// What an operation holds while it runs. `--progress-json` selects the records;
/// otherwise the human milestones. The terminal record waits for `finish`, so
/// the phase agrees with the result the operation actually reached.
pub struct Reporter<W: Write> {
    output: Output<W>,
    planned: usize,
    completed: usize,
    journalled: usize,
    wrote: bool,
    finished: bool,
}

impl Reporter<io::Stderr> {
    /// `label` names the human milestones and `operation` the records.
    pub fn for_operation(label: &'static str, operation: &'static str) -> Self {
        let stderr = io::stderr();
        let terminal = stderr.is_terminal();
        let output = if crate::envelope::progress_json() {
            Output::Json(JsonProgress::stderr(
                &crate::envelope::tool_version(),
                &crate::envelope::operation_id(),
                operation,
            ))
        } else {
            Output::Human(ProgressReporter::with_writer(label, 0, stderr, terminal))
        };
        Self::with_output(output)
    }
}

/// An operation that turned out to have nothing to do still ends its stream.
pub fn finish_without_work(label: &'static str, operation: &'static str, code: ExitCode) {
    Reporter::for_operation(label, operation).finish(code);
}

impl<W: Write> Reporter<W> {
    fn with_output(output: Output<W>) -> Self {
        Self {
            output,
            planned: 0,
            completed: 0,
            journalled: 0,
            wrote: false,
            finished: false,
        }
    }

    /// Say work has started before anything can be counted.
    pub fn indeterminate(&mut self) {
        if let Output::Json(json) = &mut self.output {
            json.indeterminate();
            self.wrote = true;
        }
    }

    pub fn update(&mut self, event: StructuredProgress) {
        self.planned = event.planned;
        self.completed = event.completed;
        self.journalled = event.journalled;
        match &mut self.output {
            Output::Json(json) => {
                if !matches!(event.phase, ProgressPhase::Done | ProgressPhase::Error) {
                    json.update(event);
                    self.wrote = true;
                }
            }
            Output::Human(human) => match event.phase {
                ProgressPhase::Planning => human.set_total(event.planned),
                ProgressPhase::Advancing => human.update(event.completed, event.planned),
                ProgressPhase::Done | ProgressPhase::Error => human.settle(),
            },
        }
    }

    /// The counted part is over: close an open TTY line before other output.
    pub fn settle(&mut self) {
        if let Output::Human(human) = &mut self.output {
            human.settle();
        }
    }

    /// Write the terminal record, once. Its phase is the operation's final
    /// outcome: `done` only where the result reads done or nothing to do.
    pub fn finish(&mut self, code: ExitCode) {
        let phase = if crate::envelope::outcome_finished(code) {
            ProgressPhase::Done
        } else {
            ProgressPhase::Error
        };
        self.terminal(phase);
    }

    fn terminal(&mut self, phase: ProgressPhase) {
        if self.finished {
            return;
        }
        self.finished = true;
        match &mut self.output {
            Output::Json(json) => json.update(StructuredProgress {
                planned: self.planned,
                completed: self.completed,
                journalled: self.journalled,
                phase,
            }),
            Output::Human(human) => human.settle(),
        }
    }
}

impl<W: Write> Drop for Reporter<W> {
    /// Reached without `finish` only by a recovered panic. A killed process
    /// runs nothing, and its missing terminal record is how a reader knows.
    fn drop(&mut self) {
        if self.wrote {
            self.terminal(ProgressPhase::Error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(
        phase: ProgressPhase,
        planned: usize,
        completed: usize,
        journalled: usize,
    ) -> StructuredProgress {
        StructuredProgress {
            planned,
            completed,
            journalled,
            phase,
        }
    }

    #[test]
    fn records_are_bounded_and_keep_terminal_failure_counts() {
        let mut bytes = Vec::new();
        {
            let mut reporter = JsonProgress::new(&mut bytes, "version", "operation", "apply");
            reporter.update(event(ProgressPhase::Planning, 1000, 0, 0));
            for n in 1..=999 {
                reporter.update(event(ProgressPhase::Advancing, 1000, n, n));
            }
            reporter.update(event(ProgressPhase::Error, 1000, 1000, 999));
        }
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.lines().count() <= MAX_EVENTS);
        let last = text.lines().last().unwrap();
        assert!(last.contains("\"completed\":1000"));
        assert!(last.contains("\"journalled\":999"));
        assert!(last.contains("\"phase\":\"error\""));
    }

    #[test]
    fn no_op_records_are_versioned_and_not_silently_suppressed() {
        let mut bytes = Vec::new();
        let mut reporter = JsonProgress::new(&mut bytes, "version", "operation", "apply");
        reporter.update(event(ProgressPhase::Done, 0, 0, 0));
        let record = String::from_utf8(bytes).unwrap();
        assert!(record.contains("\"schema_version\":1"));
        assert!(record.contains("\"planned\":0"));
        assert!(!record.contains("path"));
    }

    #[test]
    fn schema_one_keeps_the_exact_record_sweep_consumers_read() {
        // Byte-for-byte what the writer produced before it was generalized.
        let mut bytes = Vec::new();
        let mut reporter = JsonProgress::new(&mut bytes, "0.5.4", "sweep-1-2", "apply");
        reporter.update(event(ProgressPhase::Planning, 3, 0, 0));
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "{\"schema_version\":1,\"event\":\"progress\",\"tool_version\":\"0.5.4\",\
             \"operation_id\":\"sweep-1-2\",\"operation\":\"apply\",\"phase\":\"planning\",\
             \"planned\":3,\"completed\":0,\"journalled\":0}\n"
        );
        assert_eq!(SCHEMA_VERSION, 1, "changing a field requires a new version");
        assert_eq!(FIELDS.len(), 9);
    }

    #[test]
    fn an_indeterminate_run_stays_inside_the_same_bound_and_reaches_every_milestone() {
        let mut bytes = Vec::new();
        {
            let mut reporter = JsonProgress::new(&mut bytes, "version", "operation", "extract");
            reporter.indeterminate();
            reporter.update(event(ProgressPhase::Planning, 180_000, 0, 0));
            for n in 1..=180_000 {
                reporter.update(event(ProgressPhase::Advancing, 180_000, n, 0));
            }
            reporter.update(event(ProgressPhase::Done, 180_000, 180_000, 0));
        }
        let text = String::from_utf8(bytes).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), MAX_EVENTS);
        assert!(lines[0].contains("\"phase\":\"planning\"") && lines[0].contains("\"planned\":0"));
        assert!(lines[1].contains("\"planned\":180000"));
        assert!(lines[10].contains("\"completed\":180000"), "{}", lines[10]);
    }

    #[test]
    fn jumping_counts_stay_bounded_and_never_go_backwards() {
        let mut bytes = Vec::new();
        {
            let mut reporter = JsonProgress::new(&mut bytes, "version", "operation", "extract");
            reporter.update(event(ProgressPhase::Planning, 1000, 0, 0));
            for completed in [500, 400, 500, 1000, 999, 1000] {
                reporter.update(event(ProgressPhase::Advancing, 1000, completed, 0));
            }
        }
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(text.lines().count(), 3, "{text}");
        assert!(text.lines().nth(1).unwrap().contains("\"completed\":500"));
        assert!(text.lines().nth(2).unwrap().contains("\"completed\":1000"));
    }

    #[test]
    fn each_record_reaches_the_writer_in_one_write_within_the_atomic_pipe_limit() {
        struct Sizes<'a>(&'a mut Vec<usize>);
        impl Write for Sizes<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.push(bytes.len());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut sizes = Vec::new();
        {
            let id = format!("unpack-{}-{}", u32::MAX, u128::MAX);
            let mut reporter = JsonProgress::new(Sizes(&mut sizes), "100.100.100", &id, "extract");
            reporter.update(event(
                ProgressPhase::Error,
                usize::MAX,
                usize::MAX,
                usize::MAX,
            ));
        }
        assert_eq!(sizes.len(), 1);
        // POSIX guarantees PIPE_BUF is at least 512, which is its macOS value.
        assert!(sizes[0] < 512, "{}", sizes[0]);
    }

    #[test]
    fn a_failing_output_does_not_panic_or_stop_the_operation() {
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
        }
        let mut reporter = JsonProgress::new(Closed, "version", "operation", "apply");
        reporter.update(event(ProgressPhase::Done, 1, 1, 1));
    }

    fn json_reporter(bytes: &mut Vec<u8>) -> Reporter<&mut Vec<u8>> {
        Reporter::with_output(Output::Json(JsonProgress::new(
            bytes,
            "version",
            "operation",
            "pop",
        )))
    }

    #[test]
    fn the_engine_terminal_waits_for_the_outcome_and_is_written_once() {
        crate::envelope::begin("stash", "1", &["pop".into()]);
        let mut bytes = Vec::new();
        {
            let mut reporter = json_reporter(&mut bytes);
            reporter.update(event(ProgressPhase::Planning, 4, 0, 0));
            reporter.update(event(ProgressPhase::Done, 4, 3, 2));
            reporter.finish(ExitCode::from(3));
            reporter.finish(ExitCode::SUCCESS);
        }
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(text.lines().count(), 2, "{text}");
        let last = text.lines().last().unwrap();
        assert!(last.contains("\"phase\":\"error\""), "{last}");
        assert!(last.contains("\"completed\":3") && last.contains("\"journalled\":2"));
    }

    #[test]
    fn done_is_withheld_when_the_result_reads_incomplete_despite_exit_zero() {
        crate::envelope::begin("stash", "1", &["pop".into()]);
        crate::envelope::status("incomplete");
        let mut bytes = Vec::new();
        {
            let mut reporter = json_reporter(&mut bytes);
            reporter.update(event(ProgressPhase::Planning, 2, 0, 0));
            reporter.finish(ExitCode::SUCCESS);
        }
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.lines().last().unwrap().contains("\"phase\":\"error\""));
    }

    #[test]
    fn nothing_to_do_finishes_as_done_with_zero_counts() {
        crate::envelope::begin("stash", "1", &["pop".into()]);
        let mut bytes = Vec::new();
        json_reporter(&mut bytes).finish(ExitCode::from(1));
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("\"phase\":\"done\"") && text.contains("\"planned\":0"));
    }

    #[test]
    fn a_recovered_panic_ends_a_started_stream_with_the_last_counts() {
        let mut bytes = Vec::new();
        {
            let mut reporter = json_reporter(&mut bytes);
            reporter.indeterminate();
            reporter.update(event(ProgressPhase::Planning, 5, 0, 0));
            reporter.update(event(ProgressPhase::Advancing, 5, 2, 0));
        }
        let text = String::from_utf8(bytes).unwrap();
        let last = text.lines().last().unwrap();
        assert!(last.contains("\"phase\":\"error\"") && last.contains("\"completed\":2"));
    }

    #[test]
    fn a_stream_that_never_started_stays_silent() {
        let mut bytes = Vec::new();
        drop(json_reporter(&mut bytes));
        assert!(bytes.is_empty());
    }

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
    fn human_milestones_learn_the_total_from_the_plan_and_survive_jumps() {
        let mut output = Vec::new();
        {
            let mut reporter = Reporter::with_output(Output::Human(ProgressReporter::with_writer(
                "unpack",
                0,
                &mut output,
                false,
            )));
            reporter.update(event(ProgressPhase::Planning, 1000, 0, 0));
            for completed in [700, 700, 650, 1000, 1000] {
                reporter.update(event(ProgressPhase::Advancing, 1000, completed, 0));
            }
            reporter.indeterminate();
        }
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "unpack: 700/1000 items\nunpack: 1000/1000 items\n"
        );
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
