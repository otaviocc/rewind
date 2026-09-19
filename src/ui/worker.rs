//! Everything spawned off the UI thread: the `Wake` channel and its payloads, the
//! long-lived loader (the sole owner of the `SessionLru`), and the background corpus-build
//! coordinator that fans out over a pool.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ratatui::crossterm::event;

use crate::domain::cache::build::{self, Control};
use crate::domain::cache::shard::Kind;
use crate::domain::cache::store::{self, Outcome, Plan};
use crate::domain::cancel::Cancel;
use crate::domain::project::{self, Project, ProjectError};
use crate::domain::search::corpus::Corpus;
use crate::domain::search::engine::Hit;
use crate::domain::search::resolve::{self, Opened};
use crate::domain::session::{self, Session};
use crate::domain::subagent::{self, Agents};
use crate::domain::thread::{self, Conversation, ThreadError};
use crate::domain::tool;
use crate::ui::lru::{self, SessionLru};

const SCAN_PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

pub enum Wake {
    Input(event::Event),
    ProjectsLoaded { generation: u64, result: Result<Vec<Project>, ProjectError> },
    SessionsLoaded { generation: u64, sessions: Vec<Session> },
    ConversationLoaded { generation: u64, result: Result<Arc<Conversation>, ThreadError>, agents: Box<Agents> },
    ToolOutputLoaded { generation: u64, id: Box<str>, result: Result<Vec<String>, String> },
    SubagentLoaded { generation: u64, path: PathBuf, result: Result<Arc<Conversation>, ThreadError> },
    ScanProgress { done: usize, total: usize },
    ScanFinished,
    CorpusLoaded(Arc<Corpus>),
    HitResolved { generation: u64, target: Option<Opened> },
    InputLost(String),
}

pub enum Job {
    Conversation { path: PathBuf, generation: u64, cancel: Cancel },
    Subagent { path: PathBuf, generation: u64, cancel: Cancel },
    ToolOutput { id: Box<str>, path: PathBuf, generation: u64 },
}

pub fn spawn_input(tx: Sender<Wake>) {
    std::thread::spawn(move || {
        loop {
            let wake = match event::read() {
                Ok(read) => Wake::Input(read),
                Err(error) => {
                    let _ = tx.send(Wake::InputLost(error.to_string()));
                    return;
                }
            };
            if tx.send(wake).is_err() {
                return;
            }
        }
    });
}

pub fn spawn_projects_load(tx: &Sender<Wake>, claude_dir: PathBuf, generation: u64) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        let result = project::discover(&claude_dir);
        let _ = tx.send(Wake::ProjectsLoaded { generation, result });
    });
}

pub fn spawn_sessions_load(tx: &Sender<Wake>, project_dir: PathBuf, generation: u64) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        let sessions = session::discover(&project_dir);
        let _ = tx.send(Wake::SessionsLoaded { generation, sessions });
    });
}

pub fn spawn_loader(rx: Receiver<Job>, tx: Sender<Wake>) {
    std::thread::spawn(move || {
        let mut lru = SessionLru::new(lru::BUDGET_BYTES);
        while let Ok(first) = rx.recv() {
            let mut batch = vec![first];
            while let Ok(job) = rx.try_recv() {
                batch.push(job);
            }
            for job in pick(batch) {
                process(job, &mut lru, &tx);
            }
        }
    });
}

fn pick(jobs: Vec<Job>) -> Vec<Job> {
    let mut conversation = None;
    let mut subagent = None;
    let mut tool_output: Vec<(Box<str>, Job)> = Vec::new();

    for job in jobs {
        match job {
            Job::Conversation { .. } => conversation = Some(job),
            Job::Subagent { .. } => subagent = Some(job),
            Job::ToolOutput { ref id, .. } => {
                let id = id.clone();
                tool_output.retain(|(existing, _)| *existing != id);
                tool_output.push((id, job));
            }
        }
    }

    conversation.into_iter().chain(subagent).chain(tool_output.into_iter().map(|(_, job)| job)).collect()
}

fn process(job: Job, lru: &mut SessionLru, tx: &Sender<Wake>) {
    match job {
        Job::Conversation { path, generation, cancel } => {
            let agents = Box::new(subagent::discover(&path));
            let result = load_conversation(&path, &cancel, lru);
            let _ = tx.send(Wake::ConversationLoaded { generation, result, agents });
        }
        Job::Subagent { path, generation, cancel } => {
            let result = load_conversation(&path, &cancel, lru);
            let _ = tx.send(Wake::SubagentLoaded { generation, path, result });
        }
        Job::ToolOutput { id, path, generation } => {
            let result = tool::read_overflow(&path);
            let _ = tx.send(Wake::ToolOutputLoaded { generation, id, result });
        }
    }
}

fn load_conversation(path: &Path, cancel: &Cancel, lru: &mut SessionLru) -> Result<Arc<Conversation>, ThreadError> {
    let Ok(metadata) = fs::metadata(path) else { return thread::build_with(path, cancel).map(Arc::new) };
    let len = metadata.len();
    let mtime_ms = build::mtime_ms_of(&metadata);

    if let Some(cached) = lru.get(path, len, mtime_ms) {
        return Ok(cached);
    }
    let conversation = Arc::new(thread::build_with(path, cancel)?);
    lru.insert(path, len, mtime_ms, Arc::clone(&conversation));
    Ok(conversation)
}

pub fn spawn_scan(tx: &Sender<Wake>, claude_dir: PathBuf, cache_root: PathBuf, selected: Option<String>, cancel: Cancel) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        let Ok(plan) = store::prepare(&claude_dir, &cache_root) else { return };

        let mut projects = project::discover(&claude_dir).unwrap_or_default();
        order_projects(&mut projects, selected.as_deref());
        let total = projects.len();

        let (job_tx, job_rx) = mpsc::channel::<Project>();
        for project in projects {
            let _ = job_tx.send(project);
        }
        drop(job_tx);
        let job_rx = Arc::new(Mutex::new(job_rx));

        let workers = std::thread::available_parallelism().map_or(1, std::num::NonZero::get).min(8);
        let done = Arc::new(AtomicUsize::new(0));
        let outcomes: Arc<Mutex<Vec<Outcome>>> = Arc::new(Mutex::new(Vec::new()));

        std::thread::scope(|scope| {
            for _ in 0..workers {
                let job_rx = Arc::clone(&job_rx);
                let outcomes = Arc::clone(&outcomes);
                let done = Arc::clone(&done);
                let plan = &plan;
                let cancel = cancel.clone();
                let tx = tx.clone();
                scope.spawn(move || scan_worker(&job_rx, plan, &cancel, &outcomes, &done, total, &tx));
            }
        });

        if cancel.cancelled() {
            return;
        }

        let mut outcomes = outcomes.lock().map(|mut guard| std::mem::take(&mut *guard)).unwrap_or_default();
        let mut tick = || {};
        let mut control = Control::new(cancel.clone(), &mut tick);
        if let Some(history) = store::build_history(&plan, &mut control) {
            outcomes.push(history);
        }

        store::finish(&plan, total, outcomes, Duration::default());
        let _ = tx.send(Wake::ScanFinished);
    });
}

#[allow(clippy::too_many_arguments)]
fn scan_worker(
    job_rx: &Mutex<Receiver<Project>>,
    plan: &Plan,
    cancel: &Cancel,
    outcomes: &Mutex<Vec<Outcome>>,
    done: &AtomicUsize,
    total: usize,
    tx: &Sender<Wake>,
) {
    let mut last_report = Instant::now();
    let mut reported = false;
    loop {
        if cancel.cancelled() {
            return;
        }
        let next = { job_rx.lock().unwrap_or_else(std::sync::PoisonError::into_inner).recv() };
        let Ok(project) = next else { return };

        let mut tick = || {};
        let mut control = Control::new(cancel.clone(), &mut tick);
        let outcome = store::build_project(plan, &project, &mut control);
        if let Ok(mut outcomes) = outcomes.lock() {
            outcomes.push(outcome);
        }

        let now_done = done.fetch_add(1, Ordering::AcqRel).saturating_add(1);
        if !reported || last_report.elapsed() >= SCAN_PROGRESS_INTERVAL || now_done == total {
            let _ = tx.send(Wake::ScanProgress { done: now_done, total });
            last_report = Instant::now();
            reported = true;
        }
    }
}

fn order_projects(projects: &mut [Project], selected: Option<&str>) {
    projects.sort_by_key(|project| std::cmp::Reverse(project.last_activity));
    let Some(selected) = selected else { return };
    let Some(index) = projects.iter().position(|project| project.directory == selected) else { return };
    projects.swap(0, index);
}

pub fn spawn_corpus_load(tx: &Sender<Wake>, cache_root: PathBuf) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        let corpus = crate::domain::search::corpus::load(&cache_root);
        let _ = tx.send(Wake::CorpusLoaded(Arc::new(corpus)));
    });
}

pub fn spawn_resolve_hit(tx: &Sender<Wake>, claude_dir: PathBuf, hit: Hit, generation: u64) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        let target = resolve_hit(&claude_dir, &hit);
        let _ = tx.send(Wake::HitResolved { generation, target });
    });
}

fn resolve_hit(claude_dir: &Path, hit: &Hit) -> Option<Opened> {
    match hit.kind {
        Kind::History => {
            let (session_id, cwd) = resolve::resolve_history(claude_dir, hit)?;
            Some(Opened { project_directory: project::encode(&cwd), session_id, uuid: None, agent_id: None })
        }
        Kind::Transcript | Kind::Subagent => resolve::resolve_transcript(claude_dir, hit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation_job(generation: u64) -> Job {
        Job::Conversation { path: PathBuf::from("/a"), generation, cancel: Cancel::never() }
    }

    fn tool_output_job(id: &str, generation: u64) -> Job {
        Job::ToolOutput { id: Box::from(id), path: PathBuf::from("/a"), generation }
    }

    #[test]
    fn picking_a_batch_keeps_only_the_newest_conversation_job() {
        let jobs = vec![conversation_job(1), conversation_job(2), conversation_job(3)];
        let picked = pick(jobs);
        assert_eq!(picked.len(), 1);
        assert!(matches!(picked.first(), Some(Job::Conversation { generation: 3, .. })));
    }

    #[test]
    fn picking_a_batch_keeps_one_conversation_and_one_subagent_job() {
        let jobs = vec![
            conversation_job(1),
            Job::Subagent { path: PathBuf::from("/b"), generation: 1, cancel: Cancel::never() },
            conversation_job(2),
        ];
        let picked = pick(jobs);
        assert_eq!(picked.len(), 2);
    }

    #[test]
    fn picking_a_batch_dedupes_tool_output_by_id_keeping_the_last() {
        let jobs = vec![tool_output_job("a", 1), tool_output_job("b", 1), tool_output_job("a", 2)];
        let picked = pick(jobs);
        assert_eq!(picked.len(), 2);
        let a = picked.iter().find(|job| matches!(job, Job::ToolOutput { id, .. } if &**id == "a"));
        assert!(matches!(a, Some(Job::ToolOutput { generation: 2, .. })));
    }

    #[test]
    fn ordering_projects_puts_the_selected_one_first_regardless_of_recency() {
        use std::time::{Duration, SystemTime};

        let mut projects = vec![
            Project {
                directory: "old".to_owned(),
                path: PathBuf::from("/old"),
                resolution: project::Resolution::Mapped,
                sessions: 1,
                last_activity: SystemTime::UNIX_EPOCH,
                present: true,
            },
            Project {
                directory: "new".to_owned(),
                path: PathBuf::from("/new"),
                resolution: project::Resolution::Mapped,
                sessions: 1,
                last_activity: SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(100)).unwrap_or(SystemTime::UNIX_EPOCH),
                present: true,
            },
        ];

        order_projects(&mut projects, Some("old"));
        assert_eq!(projects.first().map(|project| project.directory.as_str()), Some("old"));
    }

    #[test]
    fn ordering_projects_with_no_selection_sorts_by_descending_recency() {
        use std::time::{Duration, SystemTime};

        let mut projects = vec![
            Project {
                directory: "old".to_owned(),
                path: PathBuf::from("/old"),
                resolution: project::Resolution::Mapped,
                sessions: 1,
                last_activity: SystemTime::UNIX_EPOCH,
                present: true,
            },
            Project {
                directory: "new".to_owned(),
                path: PathBuf::from("/new"),
                resolution: project::Resolution::Mapped,
                sessions: 1,
                last_activity: SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(100)).unwrap_or(SystemTime::UNIX_EPOCH),
                present: true,
            },
        ];

        order_projects(&mut projects, None);
        assert_eq!(projects.first().map(|project| project.directory.as_str()), Some("new"));
    }
}
