//! Live badges: `~/.claude/sessions/<pid>.json`, proved alive rather than merely present.
//!
//! That directory is a registry of running processes, not session storage, and its entries
//! linger after the process behind them dies. A pid alone is not proof of life.

use std::fs;
use std::path::Path;
#[cfg(not(unix))]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

#[cfg(not(unix))]
const FRESHNESS: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Busy,
    Idle,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Live {
    pub pid: u32,
    pub session_id: String,
    pub status: Status,
    pub waiting_for: Option<String>,
    pub name: Option<String>,
}

impl Live {
    pub fn describe(&self) -> String {
        if let Some(waiting) = &self.waiting_for {
            return format!("waiting for {waiting}");
        }
        match self.status {
            Status::Busy => "busy".to_owned(),
            Status::Idle => "idle".to_owned(),
            Status::Unknown => "running".to_owned(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Record {
    pid: u32,
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "waitingFor")]
    waiting_for: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "updatedAt")]
    updated_at: Option<i64>,
}

pub fn scan(claude_dir: &Path) -> Vec<Live> {
    scan_with(claude_dir, &real_alive)
}

pub fn scan_with(claude_dir: &Path, alive: &dyn Fn(u32, Option<i64>) -> bool) -> Vec<Live> {
    let Ok(entries) = fs::read_dir(claude_dir.join("sessions")) else { return Vec::new() };

    let mut live: Vec<Live> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .filter_map(|path| fs::read(path).ok())
        .filter_map(|bytes| serde_json::from_slice::<Record>(&bytes).ok())
        .filter(|record| alive(record.pid, record.updated_at))
        .map(|record| Live {
            pid: record.pid,
            session_id: record.session_id,
            status: status_of(record.status.as_deref()),
            waiting_for: record.waiting_for,
            name: record.name,
        })
        .collect();
    live.sort_by(|left, right| left.session_id.cmp(&right.session_id));
    live
}

fn status_of(raw: Option<&str>) -> Status {
    match raw {
        Some("busy") => Status::Busy,
        Some("idle") => Status::Idle,
        _ => Status::Unknown,
    }
}

#[cfg(unix)]
fn real_alive(pid: u32, _updated_at: Option<i64>) -> bool {
    let Ok(raw) = i32::try_from(pid) else { return false };
    let Some(pid) = rustix::process::Pid::from_raw(raw) else { return false };
    match rustix::process::test_kill_process(pid) {
        Ok(()) => true,
        Err(errno) => errno == rustix::io::Errno::PERM,
    }
}

#[cfg(not(unix))]
fn real_alive(_pid: u32, updated_at: Option<i64>) -> bool {
    let Some(updated_at) = updated_at else { return false };
    let Ok(millis) = u64::try_from(updated_at) else { return false };
    let Some(at) = UNIX_EPOCH.checked_add(Duration::from_millis(millis)) else { return false };
    SystemTime::now().duration_since(at).is_ok_and(|elapsed| elapsed <= FRESHNESS)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn write(dir: &Path, name: &str, contents: &str) {
        fs::write(dir.join(name), contents).expect("a written fixture");
    }

    fn sessions_dir() -> TempDir {
        let root = TempDir::new().expect("a temporary directory");
        fs::create_dir(root.path().join("sessions")).expect("a sessions directory");
        root
    }

    #[test]
    fn a_dead_pid_is_dropped_even_though_its_file_still_says_busy() {
        let root = sessions_dir();
        write(root.path(), "sessions/999999.json", r#"{"pid":999999,"sessionId":"dead","status":"busy"}"#);
        let live = scan_with(root.path(), &|_, _| false);
        assert!(live.is_empty());
    }

    #[test]
    fn a_live_pid_carries_its_session_id_and_status() {
        let root = sessions_dir();
        write(
            root.path(),
            "sessions/4101.json",
            r#"{"pid":4101,"sessionId":"11111111-1111-4111-8111-111111111111","status":"busy","name":"holodeck-grid-scanner"}"#,
        );
        let live = scan_with(root.path(), &|_, _| true);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].pid, 4101);
        assert_eq!(live[0].session_id, "11111111-1111-4111-8111-111111111111");
        assert_eq!(live[0].status, Status::Busy);
        assert_eq!(live[0].name.as_deref(), Some("holodeck-grid-scanner"));
    }

    #[test]
    fn waiting_for_input_survives_the_round_trip() {
        let root = sessions_dir();
        write(root.path(), "sessions/4200.json", r#"{"pid":4200,"sessionId":"waiting","status":"idle","waitingFor":"input"}"#);
        let live = scan_with(root.path(), &|_, _| true);
        assert_eq!(live[0].waiting_for.as_deref(), Some("input"));
    }

    #[test]
    fn the_opaque_key_blob_is_ignored_rather_than_parsed() {
        let root = sessions_dir();
        write(root.path(), "sessions/4101.json", r#"{"pid":4101,"sessionId":"s","status":"busy"}"#);
        write(root.path(), "sessions/4101.abababab.key", "not json at all");
        let live = scan_with(root.path(), &|_, _| true);
        assert_eq!(live.len(), 1);
    }

    #[test]
    fn a_status_the_schema_does_not_name_yet_is_unknown_rather_than_a_panic() {
        let root = sessions_dir();
        write(root.path(), "sessions/4101.json", r#"{"pid":4101,"sessionId":"s","status":"sleeping"}"#);
        let live = scan_with(root.path(), &|_, _| true);
        assert_eq!(live[0].status, Status::Unknown);
    }

    #[test]
    fn a_missing_sessions_directory_is_no_live_badges_rather_than_an_error() {
        let root = TempDir::new().expect("a temporary directory");
        assert!(scan_with(root.path(), &|_, _| true).is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn the_real_liveness_check_confirms_this_very_process_is_alive() {
        let root = sessions_dir();
        let pid = std::process::id();
        write(root.path(), "sessions/alive.json", &format!(r#"{{"pid":{pid},"sessionId":"alive","status":"busy"}}"#));

        let live = scan(root.path());
        assert!(live.iter().any(|entry| entry.session_id == "alive"), "{live:?}");
    }

    #[test]
    fn results_are_ordered_by_session_id_for_a_deterministic_join() {
        let root = sessions_dir();
        write(root.path(), "sessions/1.json", r#"{"pid":1,"sessionId":"b","status":"idle"}"#);
        write(root.path(), "sessions/2.json", r#"{"pid":2,"sessionId":"a","status":"idle"}"#);
        let live = scan_with(root.path(), &|_, _| true);
        assert_eq!(live.iter().map(|entry| entry.session_id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    }

    fn live(status: Status, waiting_for: Option<&str>) -> Live {
        Live { pid: 1, session_id: "s".to_owned(), status, waiting_for: waiting_for.map(str::to_owned), name: None }
    }

    #[test]
    fn waiting_for_input_reads_as_waiting_for_input_rather_than_the_generic_status() {
        assert_eq!(live(Status::Idle, Some("input")).describe(), "waiting for input");
    }

    #[test]
    fn busy_and_idle_read_as_themselves() {
        assert_eq!(live(Status::Busy, None).describe(), "busy");
        assert_eq!(live(Status::Idle, None).describe(), "idle");
    }

    #[test]
    fn a_status_the_schema_does_not_name_yet_still_reads_as_something() {
        assert_eq!(live(Status::Unknown, None).describe(), "running");
    }
}
