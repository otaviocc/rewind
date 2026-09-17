//! Project discovery against the fixture tree: the lossy directory name, resolved back.

#![allow(clippy::expect_used)]

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use common::{FixtureTree, fixture_tree, fixtures, rewind};
use rewind::domain::project::{Project, Resolution, discover, map_path};

fn discovered(tree: &FixtureTree) -> BTreeMap<String, Project> {
    discover(&tree.claude_dir())
        .expect("the fixture tree has a projects directory")
        .into_iter()
        .map(|project| (project.directory.clone(), project))
        .collect()
}

fn expect<'a>(projects: &'a BTreeMap<String, Project>, directory: &str) -> &'a Project {
    projects.get(directory).expect("the project was discovered")
}

const EVERY_PROJECT: [&str; 7] = [
    ".tricorder",
    "Developer/holodeck",
    "Developer/jeffries-tube",
    "Developer/nomad",
    "Developer/shuttlebay",
    "Developer/warp-core",
    "Music/Red Alert - Live (2019)",
];

#[test]
fn every_project_directory_is_discovered_and_the_ds_store_is_not_one_of_them() {
    let tree = fixture_tree();
    let projects = discovered(&tree);

    let mut expected: Vec<String> = EVERY_PROJECT.iter().map(|relative| tree.project_dir(relative)).collect();
    expected.sort();
    assert_eq!(projects.keys().cloned().collect::<Vec<_>>(), expected);
    assert!(tree.claude_dir().join("projects").join(".DS_Store").is_file(), "the non-directory entry vanished from the fixture");
}

#[test]
fn the_checkout_resolves_every_directory_name_straight_from_the_map() {
    let projects: BTreeMap<String, Project> = discover(&fixtures())
        .expect("the checkout has a projects directory")
        .into_iter()
        .map(|project| (project.directory.clone(), project))
        .collect();

    assert_eq!(expect(&projects, "-Users-fixture--tricorder").path, PathBuf::from("/Users/fixture/.tricorder"));
    assert_eq!(expect(&projects, "-Users-fixture-Developer-holodeck").resolution, Resolution::Mapped);
    assert_eq!(
        expect(&projects, "-Users-fixture-Music-Red-Alert---Live-(2019)").path,
        PathBuf::from("/Users/fixture/Music/Red Alert - Live (2019)")
    );
    assert!(projects.values().all(|project| !project.present), "every fixture path is meant to be gone in the checkout");
}

#[test]
fn a_single_matching_key_resolves_straight_from_the_map() {
    let tree = fixture_tree();
    let projects = discovered(&tree);
    let holodeck = expect(&projects, &tree.project_dir("Developer/holodeck"));

    assert_eq!(holodeck.path, tree.working_copy("Developer/holodeck"));
    assert_eq!(holodeck.resolution, Resolution::Mapped);
    assert_eq!(holodeck.sessions, 4);
    assert!(holodeck.present);
}

#[test]
fn three_colliding_keys_are_separated_by_the_cwd_on_a_transcript_record() {
    let tree = fixture_tree();
    let warp_core = expect(&discovered(&tree), &tree.project_dir("Developer/warp-core")).clone();

    assert_eq!(warp_core.path, tree.working_copy("Developer/warp-core"));
    assert_eq!(warp_core.resolution, Resolution::Disambiguated);
    assert_ne!(warp_core.path, tree.working_copy("Developer/warp.core"));
    assert_ne!(warp_core.path, tree.working_copy("Developer/warp core"));
}

#[test]
fn a_directory_absent_from_the_map_is_named_by_its_cwd_alone() {
    let tree = fixture_tree();
    let jeffries = expect(&discovered(&tree), &tree.project_dir("Developer/jeffries-tube")).clone();

    assert_eq!(jeffries.path, tree.working_copy("Developer/jeffries-tube"));
    assert_eq!(jeffries.resolution, Resolution::FromCwd);
    assert!(jeffries.present);
}

#[test]
fn a_project_whose_working_copy_is_gone_is_flagged_and_not_dropped() {
    let tree = fixture_tree();
    let nomad = expect(&discovered(&tree), &tree.project_dir("Developer/nomad")).clone();

    assert_eq!(nomad.path, tree.working_copy("Developer/nomad"));
    assert_eq!(nomad.resolution, Resolution::Mapped);
    assert_eq!(nomad.sessions, 1);
    assert!(!nomad.present, "nomad has history but no working copy");
}

#[test]
fn a_project_directory_with_no_transcripts_survives_with_no_sessions() {
    let tree = fixture_tree();
    let shuttlebay = expect(&discovered(&tree), &tree.project_dir("Developer/shuttlebay")).clone();

    assert_eq!(shuttlebay.sessions, 0);
    assert_eq!(shuttlebay.resolution, Resolution::Mapped);
    assert!(shuttlebay.present);
}

#[test]
fn a_path_with_spaces_dashes_and_parentheses_round_trips_through_the_encoding() {
    let tree = fixture_tree();
    let music = expect(&discovered(&tree), &tree.project_dir("Music/Red Alert - Live (2019)")).clone();

    assert_eq!(music.path, tree.working_copy("Music/Red Alert - Live (2019)"));
    assert_eq!(music.resolution, Resolution::Mapped);
}

#[test]
fn the_list_is_ordered_by_last_activity_and_is_stable_across_builds() {
    let order = |tree: &FixtureTree| {
        discover(&tree.claude_dir())
            .expect("discovery")
            .into_iter()
            .map(|project| {
                project
                    .path
                    .strip_prefix(tree.home())
                    .map_or_else(|_| project.directory.clone(), |tail| tail.display().to_string())
            })
            .collect::<Vec<_>>()
    };
    let first = fixture_tree();
    let second = fixture_tree();

    let activity: Vec<_> = discover(&first.claude_dir()).expect("discovery").into_iter().map(|p| p.last_activity).collect();
    assert!(activity.windows(2).all(|pair| pair.first() >= pair.last()), "not sorted newest first: {activity:?}");
    assert_eq!(order(&first), order(&second), "the order is not deterministic");
}

#[test]
fn the_binary_prints_every_resolved_path() {
    let tree = fixture_tree();
    let output = rewind().arg("--claude-dir").arg(tree.claude_dir()).output().expect("rewind runs");
    assert!(output.status.success(), "rewind failed: {}", String::from_utf8_lossy(&output.stderr));

    let printed = String::from_utf8_lossy(&output.stdout);
    for project in discover(&tree.claude_dir()).expect("discovery") {
        let path = project.path.display().to_string();
        assert!(printed.contains(&path), "{path} is missing from:\n{printed}");
    }
}

#[test]
fn the_projects_map_is_opened_read_only_and_never_written() {
    let tree = fixture_tree();
    let map: PathBuf = map_path(&tree.claude_dir());
    let before = (fs::read(&map).expect("a readable map"), fs::metadata(&map).expect("map metadata").modified().ok());

    rewind().arg("--claude-dir").arg(tree.claude_dir()).output().expect("rewind runs");

    let after = (fs::read(&map).expect("a readable map"), fs::metadata(&map).expect("map metadata").modified().ok());
    assert_eq!(before.0, after.0, "the projects map was rewritten");
    assert_eq!(before.1, after.1, "the projects map mtime moved");
}
