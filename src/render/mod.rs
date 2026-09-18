//! Pure rendering: a `Conversation` becomes styled lines. No I/O, no terminal.

pub mod code;
pub mod divider;
pub mod injection;
pub mod line;
pub mod message;
pub mod prose;
pub mod tool;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::domain::subagent::Agents;
use crate::domain::thread::NodeId;

pub type Expanded = HashSet<Box<str>>;
pub type Outputs = HashMap<Box<str>, Overflow>;
pub type Branches = HashMap<NodeId, NodeId>;

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
    pub agents: &'a Agents,
    pub root: Option<NodeId>,
    pub branches: &'a Branches,
    pub injections: bool,
}

impl Ctx<'_> {
    pub fn is_expanded(&self, id: &str) -> bool {
        self.expanded.contains(id)
    }

    pub const fn narrowed(&self, width: usize) -> Ctx<'_> {
        Ctx {
            width,
            expanded: self.expanded,
            outputs: self.outputs,
            agents: self.agents,
            root: self.root,
            branches: self.branches,
            injections: self.injections,
        }
    }
}
