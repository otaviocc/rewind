//! Orchestration: the cache directory layout, per-project independence, and `meta.json`.
//!
//! `rebuild` is the whole cold-build/refresh path; every project shard is independent — "12
//! of 20 indexed" is this function's normal return value, not a partial failure — and nothing
//! here ever opens a file under `claude_dir` for writing.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::domain::cache::shard::{self, Kind, Shard};
use crate::domain::cache::{atomic, build, meta};
use crate::domain::lines::Lines;
use crate::domain::project::Project;
use crate::domain::scan;
use crate::domain::session;
use crate::domain::subagent;
use crate::domain::{project, text};

const HISTORY_FILE_NAME: &str = "history.jsonl";
const HISTORY_SHARD_NAME: &str = "_history";
const SHARD_EXTENSION: &str = "shard";
const META_FILE_NAME: &str = "meta.json";

#[derive(Debug, Clone)]
pub struct Report {
    pub projects_total: usize,
    pub projects_indexed: usize,
    pub shard_bytes: u64,
    pub wall: Duration,
    pub failures: Vec<(String, String)>,
    pub cold_rebuilds: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Plan {
    claude_dir: PathBuf,
    version_dir: PathBuf,
    built_at_ms: i64,
}

impl Plan {
    pub fn version_dir(&self) -> &Path {
        &self.version_dir
    }
}

pub fn shard_path(plan: &Plan, directory: Option<&str>) -> PathBuf {
    let stem = directory.unwrap_or(HISTORY_SHARD_NAME);
    plan.version_dir.join(stem).with_extension(SHARD_EXTENSION)
}

#[derive(Debug)]
pub enum Outcome {
    Project {
        directory: String,
        shard_bytes: u64,
        cold_rebuild: bool,
        cancelled: bool,
        error: Option<String>,
        sessions: Vec<meta::MetaSession>,
    },
    History {
        shard_bytes: u64,
        cold_rebuild: bool,
        cancelled: bool,
        error: Option<String>,
    },
}

struct ShardResult {
    shard_bytes: u64,
    cold_rebuild: bool,
    cancelled: bool,
    error: Option<String>,
}

pub fn prepare(claude_dir: &Path, cache_root: &Path) -> Result<Plan, String> {
    let version_dir = cache_root.join(format!("v{}", shard::CACHE_VERSION));
    fs::create_dir_all(&version_dir).map_err(|_| format!("cannot create {}", version_dir.display()))?;
    prune_other_versions(cache_root, &version_dir);
    atomic::clean_stray_tmp_files(&version_dir);
    Ok(Plan { claude_dir: claude_dir.to_path_buf(), version_dir, built_at_ms: now_ms() })
}

pub fn build_project(plan: &Plan, project: &Project, control: &mut build::Control<'_>) -> Outcome {
    let project_dir = plan.claude_dir.join("projects").join(&project.directory);
    let sessions = project::transcripts(&project_dir);
    let files = project_inputs(&project_dir, &sessions);
    let path = shard_path(plan, Some(&project.directory));

    let result = build_shard(&files, &path, text::extract, plan.built_at_ms, control);
    let sessions =
        if result.cancelled { Vec::new() } else { session::discover(&project_dir).into_iter().map(session_summary).collect() };

    Outcome::Project {
        directory: project.directory.clone(),
        shard_bytes: result.shard_bytes,
        cold_rebuild: result.cold_rebuild,
        cancelled: result.cancelled,
        error: result.error,
        sessions,
    }
}

fn project_inputs(project_dir: &Path, sessions: &[PathBuf]) -> Vec<build::Input> {
    let mut files: Vec<build::Input> = sessions
        .iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_owned();
            Some(build::Input { path: path.clone(), name, kind: Kind::Transcript })
        })
        .collect();

    for session_path in sessions {
        for agent in subagent::discover(session_path).all() {
            let Some(transcript) = &agent.transcript else { continue };
            let Some(name) = relative_name(project_dir, transcript) else { continue };
            files.push(build::Input { path: transcript.clone(), name, kind: Kind::Subagent });
        }
    }
    files
}

fn relative_name(base: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(base).ok()?;
    let parts: Vec<&str> = relative.components().filter_map(|part| part.as_os_str().to_str()).collect();
    if parts.is_empty() { None } else { Some(parts.join("/")) }
}

pub fn build_history(plan: &Plan, control: &mut build::Control<'_>) -> Option<Outcome> {
    let history_path = plan.claude_dir.join(HISTORY_FILE_NAME);
    if !history_path.is_file() {
        return None;
    }
    let input = build::Input { path: history_path, name: HISTORY_FILE_NAME.to_owned(), kind: Kind::History };
    let path = shard_path(plan, None);
    let result = build_shard(&[input], &path, text::extract_history, plan.built_at_ms, control);
    Some(Outcome::History {
        shard_bytes: result.shard_bytes,
        cold_rebuild: result.cold_rebuild,
        cancelled: result.cancelled,
        error: result.error,
    })
}

pub fn finish(plan: &Plan, projects_total: usize, outcomes: Vec<Outcome>, wall: Duration) -> Report {
    let mut failures = Vec::new();
    let mut cold_rebuilds = Vec::new();
    let mut meta_projects = Vec::new();
    let mut shard_bytes_total: u64 = 0;
    let mut projects_indexed = 0_usize;

    for outcome in outcomes {
        match outcome {
            Outcome::Project { directory, shard_bytes, cold_rebuild, cancelled, error, sessions } => {
                if let Some(error) = error {
                    failures.push((directory, error));
                    continue;
                }
                if cancelled {
                    continue;
                }
                shard_bytes_total = shard_bytes_total.saturating_add(shard_bytes);
                projects_indexed = projects_indexed.saturating_add(1);
                if cold_rebuild {
                    cold_rebuilds.push(directory.clone());
                }
                meta_projects.push(meta::MetaProject { directory, sessions });
            }
            Outcome::History { shard_bytes, cold_rebuild, cancelled, error } => {
                if let Some(error) = error {
                    failures.push((HISTORY_SHARD_NAME.to_owned(), error));
                    continue;
                }
                if cancelled {
                    continue;
                }
                shard_bytes_total = shard_bytes_total.saturating_add(shard_bytes);
                if cold_rebuild {
                    cold_rebuilds.push(HISTORY_SHARD_NAME.to_owned());
                }
            }
        }
    }

    let built_seconds = plan.built_at_ms.checked_div(1000).unwrap_or(0);
    let meta = meta::Meta { cache_version: shard::CACHE_VERSION, built_at_s: built_seconds, projects: meta_projects };
    if let Err(error) = meta::write(&plan.version_dir.join(META_FILE_NAME), &meta) {
        failures.push((META_FILE_NAME.to_owned(), error.to_string()));
    }

    Report { projects_total, projects_indexed, shard_bytes: shard_bytes_total, wall, failures, cold_rebuilds }
}

pub fn rebuild(claude_dir: &Path, cache_root: &Path) -> Report {
    let started = Instant::now();
    let plan = match prepare(claude_dir, cache_root) {
        Ok(plan) => plan,
        Err(message) => {
            return Report {
                projects_total: 0,
                projects_indexed: 0,
                shard_bytes: 0,
                wall: started.elapsed(),
                failures: vec![("<cache>".to_owned(), message)],
                cold_rebuilds: Vec::new(),
            };
        }
    };

    let mut leading_failures = Vec::new();
    let projects = project::discover(claude_dir).unwrap_or_else(|_| {
        leading_failures.push(("<projects>".to_owned(), "cannot read the projects directory".to_owned()));
        Vec::new()
    });
    let projects_total = projects.len();

    let mut outcomes: Vec<Outcome> =
        projects.iter().map(|project| build_project(&plan, project, &mut build::Control::inert())).collect();
    if let Some(history) = build_history(&plan, &mut build::Control::inert()) {
        outcomes.push(history);
    }

    let mut report = finish(&plan, projects_total, outcomes, started.elapsed());
    report.failures.splice(0..0, leading_failures);
    report
}

pub fn purge(cache_root: &Path) {
    let _ = fs::remove_dir_all(cache_root);
}

fn build_shard(
    files: &[build::Input],
    shard_path: &Path,
    extract: fn(&[u8]) -> Vec<text::Extracted>,
    built_at_ms: i64,
    control: &mut build::Control<'_>,
) -> ShardResult {
    let previous_bytes = fs::read(shard_path).ok();
    let was_corrupt = previous_bytes.is_some();
    let previous = previous_bytes.as_deref().and_then(|bytes| Shard::parse(bytes).ok());
    let was_corrupt = was_corrupt && previous.is_none();

    let output = build::build_with(files, previous.as_ref(), extract, built_at_ms, control);
    if output.cancelled {
        return ShardResult { shard_bytes: 0, cold_rebuild: false, cancelled: true, error: None };
    }
    let len = u64::try_from(output.shard_bytes.len()).unwrap_or(u64::MAX);
    match atomic::write(shard_path, &output.shard_bytes) {
        Ok(()) => ShardResult { shard_bytes: len, cold_rebuild: was_corrupt, cancelled: false, error: None },
        Err(error) => ShardResult { shard_bytes: 0, cold_rebuild: was_corrupt, cancelled: false, error: Some(error.to_string()) },
    }
}

fn session_summary(session: session::Session) -> meta::MetaSession {
    let (cwd, roots, branches) = session_forest(&session.path);
    meta::MetaSession {
        id: session.id,
        title: session.title,
        cwd,
        git_branch: session.git_branch,
        first_activity_s: session.first_activity.map(jiff::Timestamp::as_second),
        last_activity_s: session.last_activity.map(jiff::Timestamp::as_second),
        records: session.records,
        messages: session.messages,
        roots,
        branches,
    }
}

fn session_forest(path: &Path) -> (Option<String>, u32, u32) {
    let Ok(mut lines) = Lines::open(path) else { return (None, 0, 0) };
    let mut tally = meta::ForestTally::new();
    let mut cwd = None;
    while let Ok(Some(line)) = lines.next_line() {
        tally.record(line);
        if cwd.is_none() {
            cwd = scan::top_level_str(line, "cwd").map(str::to_owned);
        }
    }
    let (roots, branches) = tally.finish();
    (cwd, roots, branches)
}

fn prune_other_versions(cache_root: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(cache_root) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let is_version_dir = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix('v'))
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|character| character.is_ascii_digit()));
        if is_version_dir {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map_or(0, |duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    fn write_line(path: &Path, line: &str) {
        fs::write(path, format!("{line}\n")).expect("a written fixture line");
    }

    const HUMAN: &str = r#"{"type":"user","message":{"role":"user","content":"read the grid scanner"},"origin":{"kind":"human"},"cwd":"/Users/fixture/holodeck","gitBranch":"main","timestamp":"2026-01-05T09:00:00Z"}"#;

    fn one_project_store() -> (TempDir, PathBuf) {
        let claude = TempDir::new().expect("a temporary directory");
        let project_dir = claude.path().join("projects").join("-Users-fixture-holodeck");
        fs::create_dir_all(&project_dir).expect("a project directory");
        write_line(&project_dir.join("s1.jsonl"), HUMAN);
        let claude_path = claude.path().to_path_buf();
        (claude, claude_path)
    }

    #[test]
    fn a_cold_build_creates_the_version_directory_a_shard_and_meta_json() {
        let (_claude_guard, claude_dir) = one_project_store();
        let cache = TempDir::new().expect("a temporary directory");

        let report = rebuild(&claude_dir, cache.path());

        assert_eq!(report.projects_total, 1);
        assert_eq!(report.projects_indexed, 1);
        assert!(report.failures.is_empty(), "{:?}", report.failures);

        let version_dir = cache.path().join(format!("v{}", shard::CACHE_VERSION));
        assert!(version_dir.join("-Users-fixture-holodeck.shard").is_file());
        assert!(version_dir.join(META_FILE_NAME).is_file());

        let meta = meta::read(&version_dir.join(META_FILE_NAME)).expect("a readable meta.json");
        assert_eq!(meta.projects.len(), 1);
        let session = meta.projects.first().and_then(|project| project.sessions.first()).expect("one session summary");
        assert_eq!(session.cwd.as_deref(), Some("/Users/fixture/holodeck"));
        assert_eq!(session.git_branch.as_deref(), Some("main"));
        assert_eq!(session.roots, 1);
    }

    #[test]
    fn other_version_directories_are_pruned_on_a_rebuild() {
        let (_claude_guard, claude_dir) = one_project_store();
        let cache = TempDir::new().expect("a temporary directory");
        fs::create_dir_all(cache.path().join("v0")).expect("a stale version directory");
        fs::create_dir_all(cache.path().join("v99")).expect("a future version directory");
        fs::write(cache.path().join("not-a-version-dir.txt"), b"keep me").expect("an unrelated file");

        rebuild(&claude_dir, cache.path());

        assert!(!cache.path().join("v0").exists());
        assert!(!cache.path().join("v99").exists());
        assert!(cache.path().join("not-a-version-dir.txt").exists());
    }

    #[test]
    fn stray_tmp_files_are_removed_on_a_rebuild() {
        let (_claude_guard, claude_dir) = one_project_store();
        let cache = TempDir::new().expect("a temporary directory");
        let version_dir = cache.path().join(format!("v{}", shard::CACHE_VERSION));
        fs::create_dir_all(&version_dir).expect("a version directory");
        fs::write(version_dir.join("stale.shard.tmp"), b"stale").expect("a stray tmp file");

        rebuild(&claude_dir, cache.path());

        assert!(!version_dir.join("stale.shard.tmp").exists());
    }

    #[test]
    fn an_unreadable_project_directory_does_not_stop_the_rest_of_the_store() {
        let claude = TempDir::new().expect("a temporary directory");
        fs::create_dir_all(claude.path().join("projects")).expect("a projects directory");
        let cache = TempDir::new().expect("a temporary directory");

        let report = rebuild(claude.path(), cache.path());

        assert_eq!(report.projects_total, 0);
        assert!(report.failures.is_empty());
        assert!(cache.path().join(format!("v{}", shard::CACHE_VERSION)).join(META_FILE_NAME).is_file());
    }

    #[test]
    fn history_jsonl_becomes_its_own_shard() {
        let (_claude_guard, claude_dir) = one_project_store();
        write_line(&claude_dir.join(HISTORY_FILE_NAME), r#"{"display":"read the grid scanner"}"#);
        let cache = TempDir::new().expect("a temporary directory");

        rebuild(&claude_dir, cache.path());

        let version_dir = cache.path().join(format!("v{}", shard::CACHE_VERSION));
        let bytes = fs::read(version_dir.join("_history.shard")).expect("a readable history shard");
        let shard = Shard::parse(&bytes).expect("a well-formed history shard");
        assert_eq!(shard.records().len(), 1);
        assert_eq!(shard.records().first().map(|record| record.kind), Some(shard::Kind::History));
    }

    #[test]
    fn a_rebuild_never_writes_anything_under_claude_dir() {
        let (_claude_guard, claude_dir) = one_project_store();
        let cache = TempDir::new().expect("a temporary directory");

        let before = snapshot(&claude_dir);
        rebuild(&claude_dir, cache.path());
        let after = snapshot(&claude_dir);

        assert_eq!(before, after, "the claude directory must be byte-for-byte unchanged");
    }

    #[test]
    fn purge_removes_the_whole_cache_root() {
        let cache = TempDir::new().expect("a temporary directory");
        fs::create_dir_all(cache.path().join("v1")).expect("a version directory");

        purge(cache.path());

        assert!(!cache.path().exists());
    }

    fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut entries = Vec::new();
        collect(dir, &mut entries);
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries
    }

    fn collect(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        let Ok(read) = fs::read_dir(dir) else { return };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, out);
            } else if let Ok(bytes) = fs::read(&path) {
                out.push((path, bytes));
            }
        }
    }

    #[test]
    fn assembling_a_build_from_prepare_project_and_finish_matches_rebuild() {
        let (_claude_guard, claude_dir) = one_project_store();
        let cache = TempDir::new().expect("a temporary directory");

        let plan = prepare(&claude_dir, cache.path()).expect("a preparable cache root");
        let projects = project::discover(&claude_dir).expect("discoverable projects");
        let outcomes: Vec<Outcome> =
            projects.iter().map(|project| build_project(&plan, project, &mut build::Control::inert())).collect();
        let report = finish(&plan, projects.len(), outcomes, Duration::from_millis(0));

        assert_eq!(report.projects_total, 1);
        assert_eq!(report.projects_indexed, 1);
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        let version_dir = cache.path().join(format!("v{}", shard::CACHE_VERSION));
        assert!(version_dir.join("-Users-fixture-holodeck.shard").is_file());
        assert!(version_dir.join(META_FILE_NAME).is_file());
    }

    #[test]
    fn a_cancelled_project_build_is_dropped_from_meta_and_not_counted_as_indexed() {
        let (_claude_guard, claude_dir) = one_project_store();
        let cache = TempDir::new().expect("a temporary directory");

        let plan = prepare(&claude_dir, cache.path()).expect("a preparable cache root");
        let projects = project::discover(&claude_dir).expect("discoverable projects");
        let project = projects.first().expect("one project");

        let gate = crate::domain::cancel::Gate::default();
        let token = gate.token();
        gate.bump();
        let mut noop = || {};
        let mut control = build::Control::new(token, &mut noop);

        let outcome = build_project(&plan, project, &mut control);
        assert!(matches!(&outcome, Outcome::Project { cancelled: true, sessions, .. } if sessions.is_empty()));

        let report = finish(&plan, projects.len(), vec![outcome], Duration::from_millis(0));
        assert_eq!(report.projects_indexed, 0);
        assert!(report.failures.is_empty());

        let version_dir = cache.path().join(format!("v{}", shard::CACHE_VERSION));
        let meta = meta::read(&version_dir.join(META_FILE_NAME)).expect("a readable meta.json");
        assert!(meta.projects.is_empty(), "a cancelled build must not claim the project was indexed");
    }

    #[test]
    fn shard_path_names_a_project_shard_by_its_directory_and_history_by_the_reserved_stem() {
        let (_claude_guard, claude_dir) = one_project_store();
        let cache = TempDir::new().expect("a temporary directory");
        let plan = prepare(&claude_dir, cache.path()).expect("a preparable cache root");

        let version_dir = cache.path().join(format!("v{}", shard::CACHE_VERSION));
        assert_eq!(shard_path(&plan, Some("-a-project")), version_dir.join("-a-project.shard"));
        assert_eq!(shard_path(&plan, None), version_dir.join(format!("{HISTORY_SHARD_NAME}.shard")));
    }

    #[test]
    fn build_history_returns_none_when_there_is_no_history_file() {
        let (_claude_guard, claude_dir) = one_project_store();
        let cache = TempDir::new().expect("a temporary directory");
        let plan = prepare(&claude_dir, cache.path()).expect("a preparable cache root");

        assert!(build_history(&plan, &mut build::Control::inert()).is_none());
    }
}
