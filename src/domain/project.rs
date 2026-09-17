//! Project discovery: the directories under `projects/`, resolved back to real paths.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;
use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};
use thiserror::Error;

use crate::domain::lines::Lines;
use crate::domain::scan;

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("cannot read {0}")]
    Unreadable(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Mapped,
    Disambiguated,
    FromCwd,
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub directory: String,
    pub path: PathBuf,
    pub resolution: Resolution,
    pub sessions: usize,
    pub last_activity: SystemTime,
    pub present: bool,
}

#[derive(Deserialize)]
struct Map {
    #[serde(default)]
    projects: Keys,
}

#[derive(Default)]
struct Keys(Vec<String>);

impl<'de> Deserialize<'de> for Keys {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OnlyTheKeys;

        impl<'de> Visitor<'de> for OnlyTheKeys {
            type Value = Keys;

            fn expecting(&self, formatter: &mut core::fmt::Formatter) -> core::fmt::Result {
                formatter.write_str("the projects map")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Keys, A::Error> {
                let mut keys = Vec::new();
                while let Some(key) = access.next_key::<String>()? {
                    access.next_value::<IgnoredAny>()?;
                    keys.push(key);
                }
                Ok(Keys(keys))
            }
        }

        deserializer.deserialize_map(OnlyTheKeys)
    }
}

pub fn encode(path: &str) -> String {
    path.chars().map(|character| if matches!(character, '/' | '.' | ' ') { '-' } else { character }).collect()
}

pub fn map_path(claude_dir: &Path) -> PathBuf {
    claude_dir.parent().unwrap_or(claude_dir).join(".claude.json")
}

pub fn projects_map(claude_dir: &Path) -> BTreeMap<String, Vec<PathBuf>> {
    let Ok(text) = fs::read_to_string(map_path(claude_dir)) else {
        return BTreeMap::new();
    };
    let Ok(map) = serde_json::from_str::<Map>(&text) else {
        return BTreeMap::new();
    };

    let mut grouped: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for key in map.projects.0 {
        grouped.entry(encode(&key)).or_default().push(PathBuf::from(key));
    }
    for candidates in grouped.values_mut() {
        candidates.sort();
    }
    grouped
}

pub fn discover(claude_dir: &Path) -> Result<Vec<Project>, ProjectError> {
    let root = claude_dir.join("projects");
    let entries = fs::read_dir(&root).map_err(|_| ProjectError::Unreadable(root.clone()))?;
    let map = projects_map(claude_dir);

    let mut projects = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let directory = entry.file_name().to_string_lossy().into_owned();
        let candidates = map.get(&directory).map_or(&[][..], Vec::as_slice);
        projects.push(resolve(&entry.path(), directory, candidates));
    }

    projects
        .sort_by(|left, right| right.last_activity.cmp(&left.last_activity).then_with(|| left.directory.cmp(&right.directory)));
    Ok(projects)
}

fn resolve(dir: &Path, directory: String, candidates: &[PathBuf]) -> Project {
    let sessions = transcripts(dir);
    let last_activity = newest(dir, &sessions);

    let (path, resolution) = match candidates {
        [only] => (only.clone(), Resolution::Mapped),
        [] => cwd_in(&sessions)
            .map_or_else(|| (PathBuf::from(&directory), Resolution::Unresolved), |cwd| (PathBuf::from(cwd), Resolution::FromCwd)),
        many => disambiguate(many, cwd_in(&sessions).as_deref(), &directory),
    };

    Project { directory, present: path.is_dir(), path, resolution, sessions: sessions.len(), last_activity }
}

fn disambiguate(candidates: &[PathBuf], cwd: Option<&str>, directory: &str) -> (PathBuf, Resolution) {
    if let Some(cwd) = cwd
        && let Some(matched) = candidates.iter().find(|candidate| candidate.as_os_str() == cwd)
    {
        return (matched.clone(), Resolution::Disambiguated);
    }
    candidates
        .first()
        .map_or_else(|| (PathBuf::from(directory), Resolution::Unresolved), |first| (first.clone(), Resolution::Unresolved))
}

pub(crate) fn transcripts(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|extension| extension == "jsonl"))
        .collect();
    found.sort();
    found
}

fn newest(dir: &Path, sessions: &[PathBuf]) -> SystemTime {
    sessions
        .iter()
        .filter_map(|path| fs::metadata(path).ok()?.modified().ok())
        .max()
        .or_else(|| fs::metadata(dir).ok()?.modified().ok())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn cwd_in(sessions: &[PathBuf]) -> Option<String> {
    let mut latest: Option<(SystemTime, &PathBuf)> = None;
    for path in sessions {
        let Ok(modified) = fs::metadata(path).and_then(|metadata| metadata.modified()) else {
            continue;
        };
        if latest.is_none_or(|(seen, _)| modified > seen) {
            latest = Some((modified, path));
        }
    }
    cwd_of(latest?.1)
}

fn cwd_of(path: &Path) -> Option<String> {
    let mut lines = Lines::open(path).ok()?;
    while let Some(line) = lines.next_line().ok()? {
        if let Some(cwd) = scan::top_level_str(line, "cwd") {
            return Some(cwd.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn tree(map: Option<&str>, dirs: &[&str]) -> TempDir {
        let home = TempDir::new().expect("a temporary directory");
        let projects = home.path().join("claude").join("projects");
        for dir in dirs {
            fs::create_dir_all(projects.join(dir)).expect("a project directory");
        }
        fs::create_dir_all(&projects).expect("a projects directory");
        if let Some(map) = map {
            fs::write(home.path().join(".claude.json"), map).expect("a written map");
        }
        home
    }

    fn claude(home: &TempDir) -> PathBuf {
        home.path().join("claude")
    }

    #[test]
    fn slashes_dots_and_spaces_all_become_one_dash() {
        assert_eq!(encode("/Users/x/Developer/rewind"), "-Users-x-Developer-rewind");
        assert_eq!(encode("/Users/x/.claude"), "-Users-x--claude");
        assert_eq!(encode("/Users/x/Music/Red Alert - Live (2019)"), "-Users-x-Music-Red-Alert---Live-(2019)");
        assert_eq!(
            encode("/Users/x/Downloads/Metallica - Discography 1983-2023 (FLAC) 88"),
            "-Users-x-Downloads-Metallica---Discography-1983-2023-(FLAC)-88"
        );
    }

    #[test]
    fn everything_that_is_not_a_slash_a_dot_or_a_space_survives_verbatim() {
        assert_eq!(encode("/Users/x/Developer/Default+"), "-Users-x-Developer-Default+");
        assert_eq!(encode("/A_b/C-d/E1"), "-A_b-C-d-E1");
    }

    #[test]
    fn the_projects_map_is_read_from_beside_the_claude_directory() {
        let home = tree(Some(r#"{"projects":{"/Users/x/one":{}}}"#), &[]);
        assert_eq!(map_path(&claude(&home)), home.path().join(".claude.json"));
        assert_eq!(projects_map(&claude(&home)).get("-Users-x-one"), Some(&vec![PathBuf::from("/Users/x/one")]));
    }

    #[test]
    fn three_keys_that_encode_alike_are_grouped_rather_than_overwriting_each_other() {
        let map = r#"{"projects":{"/a/warp-core":{},"/a/warp.core":{},"/a/warp core":{}}}"#;
        let home = tree(Some(map), &[]);
        let grouped = projects_map(&claude(&home));
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped.get("-a-warp-core").map(Vec::len), Some(3));
    }

    #[test]
    fn a_missing_projects_map_is_not_an_error() {
        let home = tree(None, &["-a-one"]);
        assert!(projects_map(&claude(&home)).is_empty());
        let projects = discover(&claude(&home)).expect("discovery succeeds without a map");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects.first().map(|project| project.resolution), Some(Resolution::Unresolved));
    }

    #[test]
    fn a_malformed_projects_map_degrades_instead_of_failing() {
        let home = tree(Some("{not json at all"), &["-a-one"]);
        assert!(projects_map(&claude(&home)).is_empty());
        assert_eq!(discover(&claude(&home)).map(|projects| projects.len()).ok(), Some(1));
    }

    #[test]
    fn an_unreadable_projects_directory_is_the_one_fatal_case() {
        let home = TempDir::new().expect("a temporary directory");
        let error = discover(&home.path().join("claude")).expect_err("no projects directory");
        assert!(matches!(error, ProjectError::Unreadable(_)));
    }

    #[test]
    fn an_entry_that_is_not_a_directory_is_skipped() {
        let home = tree(None, &["-a-one"]);
        fs::write(claude(&home).join("projects").join(".DS_Store"), []).expect("a written file");
        let projects = discover(&claude(&home)).expect("discovery succeeds");
        assert_eq!(projects.iter().map(|project| project.directory.as_str()).collect::<Vec<_>>(), ["-a-one"]);
    }

    #[test]
    fn a_tie_that_no_cwd_breaks_keeps_the_first_candidate_and_stays_unresolved() {
        let candidates = [PathBuf::from("/a/warp core"), PathBuf::from("/a/warp-core")];
        let (path, resolution) = disambiguate(&candidates, None, "-a-warp-core");
        assert_eq!(path, PathBuf::from("/a/warp core"));
        assert_eq!(resolution, Resolution::Unresolved);
    }

    #[test]
    fn a_cwd_that_matches_a_candidate_breaks_the_tie() {
        let candidates = [PathBuf::from("/a/warp core"), PathBuf::from("/a/warp-core")];
        let (path, resolution) = disambiguate(&candidates, Some("/a/warp-core"), "-a-warp-core");
        assert_eq!(path, PathBuf::from("/a/warp-core"));
        assert_eq!(resolution, Resolution::Disambiguated);
    }

    #[test]
    fn a_latch_record_carries_no_cwd_so_the_scan_reads_past_it() {
        let home = TempDir::new().expect("a temporary directory");
        let path = home.path().join("one.jsonl");
        fs::write(&path, "{\"type\":\"ai-title\",\"sessionId\":\"s\"}\n{\"type\":\"user\",\"cwd\":\"/a/one\"}\n")
            .expect("a written transcript");
        assert_eq!(cwd_of(&path).as_deref(), Some("/a/one"));
    }
}
