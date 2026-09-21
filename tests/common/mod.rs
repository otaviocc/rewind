//! The binary and the fixture tree, in an environment that cannot leak local configuration.

// Each integration binary compiles this module on its own and uses only part of it.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::prelude::*;
use filetime::FileTime;
use tempfile::TempDir;

const NOWHERE: &str = "/nonexistent-rewind-test";

const FIXTURE_HOME: &str = "/Users/fixture";
const FIXTURE_HOME_ENCODED: &str = "-Users-fixture";

const WORKING_COPIES_THAT_EXIST: [&str; 6] = [
    "Developer/holodeck",
    "Developer/warp-core",
    "Developer/jeffries-tube",
    "Developer/shuttlebay",
    ".tricorder",
    "Music/Red Alert - Live (2019)",
];

const REWRITTEN_EXTENSIONS: [&str; 3] = ["json", "jsonl", "md"];

const SECONDS_AT_2026_01_01: i64 = 1_767_225_600;

pub fn rewind() -> Command {
    let mut command = Command::cargo_bin("rewind").expect("the binary is built by the test harness");
    command.env_remove("NO_COLOR");
    command.env("HOME", NOWHERE);
    command.env("XDG_CONFIG_HOME", NOWHERE);
    command.env("XDG_CACHE_HOME", NOWHERE);
    command
}

pub fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("data")
}

pub fn fixtures() -> PathBuf {
    fixture_root().join("claude")
}

pub struct FixtureTree {
    home: TempDir,
}

impl FixtureTree {
    pub fn home(&self) -> &Path {
        self.home.path()
    }

    pub fn claude_dir(&self) -> PathBuf {
        self.home().join("claude")
    }

    pub fn working_copy(&self, relative: &str) -> PathBuf {
        join_all(self.home(), relative)
    }

    pub fn project_dir(&self, relative: &str) -> String {
        format!("{}-{}", encode(&self.home().to_string_lossy()), encode(relative))
    }
}

pub const LIVE_SESSION_ID: &str = "11111111-1111-4111-8111-111111111111";

pub fn fixture_tree() -> FixtureTree {
    let home = TempDir::new().expect("a temporary directory");
    let root = home.path().to_path_buf();

    copy_into(&fixture_root(), &root, &root);
    reencode_project_directories(&root);
    stamp_live_pid(&root);
    for relative in WORKING_COPIES_THAT_EXIST {
        fs::create_dir_all(join_all(&root, relative)).expect("a working copy directory");
    }
    stamp(&root);

    FixtureTree { home }
}

fn encode(path: &str) -> String {
    path.chars().map(|character| if matches!(character, '/' | '.' | ' ') { '-' } else { character }).collect()
}

fn reencode_project_directories(home: &Path) {
    let projects = home.join("claude").join("projects");
    let prefix = encode(&home.to_string_lossy());
    let entries: Vec<PathBuf> = fs::read_dir(&projects)
        .expect("a readable projects directory")
        .map(|entry| entry.expect("a readable entry").path())
        .collect();
    for path in entries {
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name().expect("a named directory").to_string_lossy().into_owned();
        let Some(tail) = name.strip_prefix(FIXTURE_HOME_ENCODED) else {
            continue;
        };
        fs::rename(&path, projects.join(format!("{prefix}{tail}"))).expect("a renamed project directory");
    }
}

fn stamp_live_pid(root: &Path) {
    let sessions = root.join("claude").join("sessions");
    let source = sessions.join("4101.json");
    let Ok(text) = fs::read_to_string(&source) else { return };
    let pid = std::process::id();
    let rewritten = text.replacen("\"pid\":4101", &format!("\"pid\":{pid}"), 1);
    fs::write(&source, rewritten).expect("a rewritten live-session fixture");
    fs::rename(&source, sessions.join(format!("{pid}.json"))).expect("a renamed live-session fixture");
}

fn join_all(base: &Path, relative: &str) -> PathBuf {
    relative.split('/').fold(base.to_path_buf(), |path, part| path.join(part))
}

fn copy_into(from: &Path, to: &Path, home: &Path) {
    fs::create_dir_all(to).expect("a fixture directory");
    for entry in fs::read_dir(from).expect("a readable fixture directory") {
        let entry = entry.expect("a readable fixture entry");
        let source = entry.path();
        let target = to.join(entry.file_name());
        if entry.file_type().expect("a fixture file type").is_dir() {
            copy_into(&source, &target, home);
        } else if is_rewritten(&source) {
            let text = fs::read_to_string(&source).expect("a utf-8 fixture file");
            let rewritten = text.replace(FIXTURE_HOME, &home.to_string_lossy());
            fs::write(&target, rewritten).expect("a written fixture file");
        } else {
            fs::copy(&source, &target).expect("a copied fixture file");
        }
    }
}

fn is_rewritten(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| REWRITTEN_EXTENSIONS.contains(&extension))
}

fn stamp(path: &Path) -> i64 {
    let seconds = if path.is_dir() {
        fs::read_dir(path)
            .expect("a readable directory")
            .map(|entry| stamp(&entry.expect("a readable entry").path()))
            .max()
            .unwrap_or(SECONDS_AT_2026_01_01)
    } else {
        newest_timestamp(path).unwrap_or(SECONDS_AT_2026_01_01)
    };
    filetime::set_file_mtime(path, FileTime::from_unix_time(seconds, 0)).expect("a settable mtime");
    seconds
}

fn newest_timestamp(path: &Path) -> Option<i64> {
    fs::read_to_string(path)
        .ok()?
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|record| seconds_of(record.get("timestamp")?))
        .max()
}

fn seconds_of(timestamp: &serde_json::Value) -> Option<i64> {
    match timestamp {
        serde_json::Value::String(text) => text.parse::<jiff::Timestamp>().ok().map(jiff::Timestamp::as_second),
        serde_json::Value::Number(number) => number.as_i64()?.checked_div(1000),
        _ => None,
    }
}
