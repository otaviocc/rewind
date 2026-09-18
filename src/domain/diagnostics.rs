//! One count for schema drift and reader-level defects, so the status line and the
//! diagnostics pane both read from a single source.

use std::path::{Path, PathBuf};

use crate::domain::lines::LineNote;

const STORED_LIMIT: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Defect {
    Oversized { line: u64, bytes: u64 },
    Truncated { line: u64 },
    Unparseable { line: u64, message: String },
    UnknownRecord { line: u64, kind: String },
    UnknownBlock { line: u64, kind: String },
    OrphanedParent { line: u64, uuid: String },
    SeveredCycle { line: u64, uuid: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostics {
    path: PathBuf,
    defects: Vec<Defect>,
    count: usize,
}

impl Diagnostics {
    pub fn new(path: &Path) -> Self {
        Self { path: path.to_owned(), defects: Vec::new(), count: 0 }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn defects(&self) -> &[Defect] {
        &self.defects
    }

    pub const fn count(&self) -> usize {
        self.count
    }

    pub fn push(&mut self, defect: Defect) {
        self.count = self.count.saturating_add(1);
        if self.defects.len() < STORED_LIMIT {
            self.defects.push(defect);
        }
    }

    pub fn absorb_line_notes(&mut self, notes: &[LineNote], line_of_offset: impl Fn(u64) -> u64) {
        for note in notes {
            let defect = match *note {
                LineNote::Oversized { offset, bytes } => Defect::Oversized { line: line_of_offset(offset), bytes },
                LineNote::Truncated { offset } => Defect::Truncated { line: line_of_offset(offset) },
            };
            self.push(defect);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_diagnostics_has_no_defects() {
        let diagnostics = Diagnostics::new(Path::new("session.jsonl"));
        assert_eq!(diagnostics.count(), 0);
        assert!(diagnostics.defects().is_empty());
    }

    #[test]
    fn the_count_keeps_growing_past_the_stored_limit() {
        let mut diagnostics = Diagnostics::new(Path::new("session.jsonl"));
        let limit = u64::try_from(STORED_LIMIT).expect("STORED_LIMIT fits in a u64");
        for line in 0..limit.saturating_add(10) {
            diagnostics.push(Defect::UnknownRecord { line, kind: "telemetry-latch".to_owned() });
        }
        assert_eq!(diagnostics.count(), STORED_LIMIT.saturating_add(10));
        assert_eq!(diagnostics.defects().len(), STORED_LIMIT);
    }

    #[test]
    fn line_notes_from_the_reader_are_folded_in_as_defects() {
        let mut diagnostics = Diagnostics::new(Path::new("session.jsonl"));
        let notes = [LineNote::Oversized { offset: 10, bytes: 9_000_000 }, LineNote::Truncated { offset: 20 }];
        diagnostics.absorb_line_notes(&notes, |offset| offset.saturating_div(10));
        assert_eq!(diagnostics.count(), 2);
        assert_eq!(diagnostics.defects(), [Defect::Oversized { line: 1, bytes: 9_000_000 }, Defect::Truncated { line: 2 }]);
    }
}
