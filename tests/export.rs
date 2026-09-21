//! Exporting a whole session to Markdown.

#![allow(clippy::expect_used)]

mod common;

use std::path::PathBuf;

use common::fixtures;
use rewind::domain::subagent;
use rewind::domain::thread;
use rewind::render::export;
use rewind::render::{Branches, Ctx, Expanded, Outputs};
use rewind::theme::Theme;

const HOLODECK: &str = "-Users-fixture-Developer-holodeck";
const BASELINE: &str = "11111111-1111-4111-8111-111111111111";
const COMPACTED: &str = "22222222-2222-4222-8222-222222222222";

fn session_path(session: &str) -> PathBuf {
    fixtures().join("projects").join(HOLODECK).join(format!("{session}.jsonl"))
}

fn exported(session: &str) -> String {
    let path = session_path(session);
    let conversation = thread::build(&path).expect("a built conversation");
    let agents = subagent::discover(&path);
    let theme = Theme::default();
    let expanded = Expanded::default();
    let outputs = Outputs::default();
    let branches = Branches::default();
    let ctx = Ctx {
        width: 80,
        theme: &theme,
        expanded: &expanded,
        outputs: &outputs,
        agents: &agents,
        root: None,
        branches: &branches,
        injections: false,
    };
    export::session(&conversation, &ctx)
}

#[test]
fn the_baseline_session_exports_to_markdown() {
    insta::assert_snapshot!("export-baseline", exported(BASELINE));
}

#[test]
fn a_compacted_session_exports_with_its_boundary() {
    insta::assert_snapshot!("export-compacted", exported(COMPACTED));
}
