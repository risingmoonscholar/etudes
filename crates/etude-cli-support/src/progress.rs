//! Opt-in progress records on stderr; counters describe moves and durable done records.
use etude_core::{
    apply::{ProgressPhase, StructuredProgress},
    json as j,
};
use std::io::{self, Write};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_EVENTS: usize = 12;

pub struct JsonProgress<W: Write> {
    writer: W,
    version: String,
    operation_id: String,
    operation: &'static str,
    next: usize,
    interval: usize,
    advances: usize,
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
            advances: 0,
        }
    }
    pub fn update(&mut self, event: StructuredProgress) {
        match event.phase {
            ProgressPhase::Planning => {
                if event.planned == 0 {
                    return;
                }
                self.interval = event.planned.div_ceil(10).max(1);
                self.next = self.interval;
            }
            ProgressPhase::Advancing => {
                if event.completed < self.next || self.advances >= 10 {
                    return;
                }
                self.advances += 1;
                self.next = event.completed.saturating_add(self.interval);
            }
            ProgressPhase::Done | ProgressPhase::Error => {}
        }
        let phase = match event.phase {
            ProgressPhase::Planning => "planning",
            ProgressPhase::Advancing => "advancing",
            ProgressPhase::Done => "done",
            ProgressPhase::Error => "error",
        };
        let record = j::obj(&[
            ("schema_version", j::num(SCHEMA_VERSION)),
            ("event", j::str("progress")),
            ("tool_version", j::str(&self.version)),
            ("operation_id", j::str(&self.operation_id)),
            ("operation", j::str(self.operation)),
            ("phase", j::str(phase)),
            ("planned", j::num(event.planned)),
            ("completed", j::num(event.completed)),
            ("journalled", j::num(event.journalled)),
        ]);
        let _ = writeln!(self.writer, "{record}");
        let _ = self.writer.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn records_are_bounded_and_keep_terminal_failure_counts() {
        let mut bytes = Vec::new();
        {
            let mut reporter = JsonProgress::new(&mut bytes, "version", "operation", "apply");
            reporter.update(StructuredProgress {
                planned: 1000,
                completed: 0,
                journalled: 0,
                phase: ProgressPhase::Planning,
            });
            for n in 1..=999 {
                reporter.update(StructuredProgress {
                    planned: 1000,
                    completed: n,
                    journalled: n,
                    phase: ProgressPhase::Advancing,
                });
            }
            reporter.update(StructuredProgress {
                planned: 1000,
                completed: 1000,
                journalled: 999,
                phase: ProgressPhase::Error,
            });
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
        reporter.update(StructuredProgress {
            planned: 0,
            completed: 0,
            journalled: 0,
            phase: ProgressPhase::Done,
        });
        let record = String::from_utf8(bytes).unwrap();
        assert!(record.contains("\"schema_version\":1"));
        assert!(record.contains("\"planned\":0"));
        assert!(!record.contains("path"));
    }
}
