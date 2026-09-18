//! Pure rendering: a `Conversation` becomes styled lines. No I/O, no terminal.

pub mod code;
pub mod line;
pub mod message;
pub mod prose;
pub mod tool;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub type Expanded = HashSet<Box<str>>;
pub type Outputs = HashMap<Box<str>, Overflow>;

#[derive(Debug, Clone)]
pub enum Overflow {
    Pending,
    Lines(Arc<Vec<String>>),
    Failed(String),
}

pub struct Ctx<'a> {
    pub width: usize,
    pub expanded: &'a Expanded,
    pub outputs: &'a Outputs,
}

impl Ctx<'_> {
    pub fn is_expanded(&self, id: &str) -> bool {
        self.expanded.contains(id)
    }

    pub const fn narrowed(&self, width: usize) -> Ctx<'_> {
        Ctx { width, expanded: self.expanded, outputs: self.outputs }
    }
}
