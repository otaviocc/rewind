//! Session discovery: one `.jsonl` per session, its title resolved from the latch tail, and
//! its shape counted without ever calling `serde_json` on the transcript.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use thiserror::Error;

use crate::domain::diagnostics::{Defect, Diagnostics};
use crate::domain::project;
use crate::domain::record;
use crate::domain::scan;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("cannot read {0}")]
    Unreadable(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleSource {
    CustomTitle,
    CustomTitleFile,
    AiTitle,
    AgentName,
    LastPrompt,
    FirstMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub path: PathBuf,
    pub title: String,
    pub title_source: TitleSource,
    pub slug: Option<String>,
    pub git_branch: Option<String>,
    pub first_activity: Option<Timestamp>,
    pub last_activity: Option<Timestamp>,
    pub records: u64,
    pub messages: u64,
    pub continued_in: Option<String>,
    pub diagnostics: Diagnostics,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Latches {
    custom_title: Option<String>,
    ai_title: Option<String>,
    agent_name: Option<String>,
    last_prompt: Option<String>,
    continued_in: Option<String>,
}

pub fn discover(project_dir: &Path) -> Vec<Session> {
    let mut sessions: Vec<Session> = project::transcripts(project_dir)
        .iter()
        .filter_map(|path| {
            let id = path.file_stem()?.to_str()?.to_owned();
            load_session(path, &id, project_dir).ok()
        })
        .collect();
    sessions.sort_by(|left, right| right.last_activity.cmp(&left.last_activity).then_with(|| left.id.cmp(&right.id)));
    sessions
}

#[derive(Debug, Default)]
pub(crate) struct Fields<'a> {
    record_type: Option<&'a str>,
    timestamp: Option<&'a str>,
    git_branch: Option<&'a str>,
    slug: Option<&'a str>,
    has_tool_use_result: bool,
    is_meta: bool,
    is_sidechain: bool,
    origin: Option<&'a [u8]>,
    message: Option<&'a [u8]>,
    request_id: Option<&'a str>,
    custom_title: Option<&'a str>,
    ai_title: Option<&'a str>,
    agent_name: Option<&'a str>,
    last_prompt: Option<&'a str>,
    continued_in: Option<&'a str>,
}

pub(crate) fn scan_line(line: &[u8]) -> Fields<'_> {
    let mut fields = Fields::default();
    for (name, value) in scan::entries(line) {
        match name {
            b"type" => fields.record_type = scan::as_str(&value),
            b"timestamp" => fields.timestamp = scan::as_str(&value),
            b"gitBranch" => fields.git_branch = scan::as_str(&value),
            b"slug" => fields.slug = scan::as_str(&value),
            b"toolUseResult" => fields.has_tool_use_result = true,
            b"isMeta" => fields.is_meta = scan::is_true(&value),
            b"isSidechain" => fields.is_sidechain = scan::is_true(&value),
            b"origin" => fields.origin = scan::as_object(&value),
            b"message" => fields.message = scan::as_object(&value),
            b"requestId" => fields.request_id = scan::as_str(&value),
            b"customTitle" => fields.custom_title = scan::as_str(&value),
            b"aiTitle" => fields.ai_title = scan::as_str(&value),
            b"agentName" => fields.agent_name = scan::as_str(&value),
            b"lastPrompt" => fields.last_prompt = scan::as_str(&value),
            b"continuedInSessionId" => fields.continued_in = scan::as_str(&value),
            _ => {}
        }
    }
    fields
}

fn load_session(path: &Path, id: &str, project_dir: &Path) -> Result<Session, SessionError> {
    let bytes = fs::read(path).map_err(|_| SessionError::Unreadable(path.to_path_buf()))?;

    let newlines: Vec<usize> = memchr::memchr_iter(b'\n', &bytes).collect();
    let records = u64::try_from(newlines.len()).unwrap_or(u64::MAX);

    let mut latches = Latches::default();
    let mut assistant_pairs: HashSet<(&[u8], &[u8])> = HashSet::new();
    let mut human_turns: u64 = 0;
    let mut first_human_message: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut slug: Option<String> = None;
    let mut first_activity: Option<Timestamp> = None;
    let mut last_activity: Option<Timestamp> = None;

    let mut diagnostics = Diagnostics::new(path);
    let mut start = 0usize;
    let mut line_number: u64 = 0;
    for end in newlines {
        let line = bytes.get(start..end).unwrap_or_default();
        start = end.saturating_add(1);
        line_number = line_number.saturating_add(1);
        if line.is_empty() {
            continue;
        }
        let fields = scan_line(line);
        match fields.record_type {
            None => diagnostics.push(Defect::Unparseable { line: line_number, message: "no top-level type".to_owned() }),
            Some(kind) if !record::is_known_type(kind) => {
                diagnostics.push(Defect::UnknownRecord { line: line_number, kind: kind.to_owned() });
            }
            Some(_) => {}
        }
        if fields.is_sidechain {
            continue;
        }

        if let Some(timestamp) = fields.timestamp.and_then(|text| text.parse::<Timestamp>().ok()) {
            if first_activity.is_none() {
                first_activity = Some(timestamp);
            }
            last_activity = Some(timestamp);
        }
        if let Some(branch) = fields.git_branch {
            git_branch = Some(branch.to_owned());
        }
        if let Some(value) = fields.slug {
            slug = Some(value.to_owned());
        }

        match fields.record_type {
            Some("user") => {
                if is_human_turn(&fields) {
                    human_turns = human_turns.saturating_add(1);
                    if first_human_message.is_none() {
                        first_human_message = human_message_text(&fields).map(str::to_owned);
                    }
                }
            }
            Some("assistant") => {
                let message_id = fields.message.and_then(|message| scan::top_level_str(message, "id"));
                if let (Some(message_id), Some(request_id)) = (message_id, fields.request_id) {
                    assistant_pairs.insert((message_id.as_bytes(), request_id.as_bytes()));
                }
            }
            Some("custom-title") => latches.custom_title = fields.custom_title.map(str::to_owned),
            Some("ai-title") => latches.ai_title = fields.ai_title.map(str::to_owned),
            Some("agent-name") => latches.agent_name = fields.agent_name.map(str::to_owned),
            Some("last-prompt") => {
                if let Some(prompt) = fields.last_prompt {
                    latches.last_prompt = Some(prompt.to_owned());
                }
            }
            Some("continued-in") => latches.continued_in = fields.continued_in.map(str::to_owned),
            _ => {}
        }
    }

    if !bytes.get(start..).unwrap_or_default().is_empty() {
        diagnostics.push(Defect::Truncated { line: line_number.saturating_add(1) });
    }

    let messages = human_turns.saturating_add(u64::try_from(assistant_pairs.len()).unwrap_or(u64::MAX));
    let custom_title_file = if latches.custom_title.is_none() { custom_title_file(project_dir, id) } else { None };
    let (title, title_source) = resolve_title(&latches, custom_title_file.as_deref(), first_human_message.as_deref())
        .unwrap_or_else(|| (id.to_owned(), TitleSource::FirstMessage));

    Ok(Session {
        id: id.to_owned(),
        path: path.to_owned(),
        title,
        title_source,
        slug,
        git_branch,
        first_activity,
        last_activity,
        records,
        messages,
        continued_in: latches.continued_in,
        diagnostics,
    })
}

pub(crate) fn is_human_turn(fields: &Fields<'_>) -> bool {
    if fields.has_tool_use_result || fields.is_meta {
        return false;
    }
    fields.origin.and_then(|origin| scan::top_level_str(origin, "kind")).is_none_or(|kind| kind == "human")
}

fn human_message_text<'a>(fields: &Fields<'a>) -> Option<&'a str> {
    scan::top_level_str(fields.message?, "content")
}

fn custom_title_file(project_dir: &Path, id: &str) -> Option<String> {
    let text = fs::read_to_string(project_dir.join(id).join("custom-title.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("customTitle")?.as_str().map(str::to_owned)
}

fn resolve_title(
    latches: &Latches,
    custom_title_file: Option<&str>,
    first_human_message: Option<&str>,
) -> Option<(String, TitleSource)> {
    if let Some(title) = &latches.custom_title {
        return Some((title.clone(), TitleSource::CustomTitle));
    }
    if let Some(title) = custom_title_file {
        return Some((title.to_owned(), TitleSource::CustomTitleFile));
    }
    if let Some(title) = &latches.ai_title {
        return Some((title.clone(), TitleSource::AiTitle));
    }
    if let Some(title) = &latches.agent_name {
        return Some((title.clone(), TitleSource::AgentName));
    }
    if let Some(title) = &latches.last_prompt {
        return Some((title.clone(), TitleSource::LastPrompt));
    }
    first_human_message.map(|title| (title.to_owned(), TitleSource::FirstMessage))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    const HUMAN: &str = r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"s1","isSidechain":false,"message":{"role":"user","content":"hi"},"origin":{"kind":"human"}}"#;
    const NO_ORIGIN: &str = r#"{"type":"user","uuid":"u2","parentUuid":null,"sessionId":"s1","isSidechain":false,"message":{"role":"user","content":"hi"}}"#;
    const TOOL_RESULT: &str = r#"{"type":"user","uuid":"u3","parentUuid":null,"sessionId":"s1","isSidechain":false,"message":{"role":"user","content":"ok"},"toolUseResult":{"stdout":"ok"}}"#;
    const META: &str = r#"{"type":"user","uuid":"u4","parentUuid":null,"sessionId":"s1","isSidechain":false,"isMeta":true,"message":{"role":"user","content":"caveat"}}"#;
    const PEER: &str = r#"{"type":"user","uuid":"u5","parentUuid":null,"sessionId":"s1","isSidechain":false,"message":{"role":"user","content":"hi"},"origin":{"kind":"peer"}}"#;

    #[test]
    fn both_tiers_agree_on_what_counts_as_a_human_turn() {
        for line in [HUMAN, NO_ORIGIN, TOOL_RESULT, META, PEER] {
            let fields = scan_line(line.as_bytes());
            let metadata_tier = is_human_turn(&fields);
            let record: crate::domain::record::UserRecord =
                serde_json::from_slice(line.as_bytes()).expect("a parseable user record");
            assert_eq!(metadata_tier, record.is_human_turn(), "the two tiers disagree about {line}");
        }
    }

    #[test]
    fn only_a_human_origin_without_plumbing_is_a_turn() {
        let human: crate::domain::record::UserRecord = serde_json::from_slice(HUMAN.as_bytes()).expect("a user record");
        let no_origin: crate::domain::record::UserRecord = serde_json::from_slice(NO_ORIGIN.as_bytes()).expect("a user record");
        assert!(human.is_human_turn());
        assert!(no_origin.is_human_turn(), "an absent origin is a human turn");
        for line in [TOOL_RESULT, META, PEER] {
            let record: crate::domain::record::UserRecord = serde_json::from_slice(line.as_bytes()).expect("a user record");
            assert!(!record.is_human_turn(), "{line} is plumbing, not a turn");
        }
    }

    fn one_session(id: &str, lines: &[&str]) -> TempDir {
        let project = TempDir::new().expect("a temporary directory");
        let path = project.path().join(format!("{id}.jsonl"));
        fs::write(path, lines.join("\n") + "\n").expect("a written transcript");
        project
    }

    #[test]
    fn the_last_ai_title_in_the_tail_wins_over_an_earlier_one() {
        let id = "s1";
        let project = one_session(
            id,
            &[
                r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#,
                r#"{"type":"ai-title","aiTitle":"first","sessionId":"s1"}"#,
                r#"{"type":"ai-title","aiTitle":"second","sessionId":"s1"}"#,
            ],
        );

        let sessions = discover(project.path());
        let session = sessions.first().expect("one session was discovered");
        assert_eq!(session.title, "second");
        assert_eq!(session.title_source, TitleSource::AiTitle);
    }

    #[test]
    fn a_session_with_no_title_latch_falls_back_to_the_first_human_message() {
        let id = "s2";
        let project = one_session(
            id,
            &[
                r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"only this"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s2"}"#,
            ],
        );

        let sessions = discover(project.path());
        let session = sessions.first().expect("one session was discovered");
        assert_eq!(session.title, "only this");
        assert_eq!(session.title_source, TitleSource::FirstMessage);
        assert_eq!(session.records, 1);
        assert_eq!(session.messages, 1);
    }

    #[test]
    fn a_custom_title_json_file_beats_an_ai_title_record() {
        let id = "s3";
        let project = one_session(
            id,
            &[
                r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s3"}"#,
                r#"{"type":"ai-title","aiTitle":"from the ai","sessionId":"s3"}"#,
            ],
        );
        let sidecar = project.path().join(id);
        fs::create_dir_all(&sidecar).expect("a session sidecar directory");
        fs::write(sidecar.join("custom-title.json"), r#"{"customTitle":"from the file"}"#).expect("a written sidecar");

        let sessions = discover(project.path());
        let session = sessions.first().expect("one session was discovered");
        assert_eq!(session.title, "from the file");
        assert_eq!(session.title_source, TitleSource::CustomTitleFile);
    }

    #[test]
    fn assistant_fragments_sharing_a_message_id_and_request_id_coalesce_into_one_message() {
        let id = "s4";
        let project = one_session(
            id,
            &[
                r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s4"}"#,
                r#"{"parentUuid":"u1","isSidechain":false,"message":{"id":"m1"},"apiBlockIndex":0,"requestId":"r1","type":"assistant","uuid":"u2","timestamp":"2026-01-01T00:00:01Z","sessionId":"s4"}"#,
                r#"{"parentUuid":"u2","isSidechain":false,"message":{"id":"m1"},"apiBlockIndex":1,"requestId":"r1","type":"assistant","uuid":"u3","timestamp":"2026-01-01T00:00:02Z","sessionId":"s4"}"#,
                r#"{"type":"ai-title","aiTitle":"t","sessionId":"s4"}"#,
            ],
        );

        let sessions = discover(project.path());
        let session = sessions.first().expect("one session was discovered");
        assert_eq!(session.messages, 2);
        assert_eq!(session.records, 4);
    }

    #[test]
    fn a_sidechain_record_is_not_counted_towards_messages() {
        let id = "s5";
        let project = one_session(
            id,
            &[
                r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s5"}"#,
                r#"{"parentUuid":"u1","isSidechain":true,"message":{"role":"user","content":"nested"},"type":"user","origin":{"kind":"human"},"uuid":"u2","timestamp":"2026-01-01T00:00:01Z","sessionId":"s5"}"#,
                r#"{"type":"ai-title","aiTitle":"t","sessionId":"s5"}"#,
            ],
        );

        let sessions = discover(project.path());
        let session = sessions.first().expect("one session was discovered");
        assert_eq!(session.messages, 1);
    }

    #[test]
    fn continued_in_is_read_from_its_own_latch() {
        let id = "s6";
        let project = one_session(
            id,
            &[
                r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s6"}"#,
                r#"{"type":"continued-in","continuedInSessionId":"successor","sessionId":"s6"}"#,
            ],
        );

        let sessions = discover(project.path());
        let session = sessions.first().expect("one session was discovered");
        assert_eq!(session.continued_in.as_deref(), Some("successor"));
    }

    fn latches(
        custom_title: Option<&str>,
        ai_title: Option<&str>,
        agent_name: Option<&str>,
        last_prompt: Option<&str>,
    ) -> Latches {
        Latches {
            custom_title: custom_title.map(str::to_owned),
            ai_title: ai_title.map(str::to_owned),
            agent_name: agent_name.map(str::to_owned),
            last_prompt: last_prompt.map(str::to_owned),
            continued_in: None,
        }
    }

    #[test]
    fn a_custom_title_record_outranks_every_other_tier() {
        let latches = latches(Some("record wins"), Some("ai"), Some("agent"), Some("prompt"));
        assert_eq!(
            resolve_title(&latches, Some("file"), Some("first message")),
            Some(("record wins".to_owned(), TitleSource::CustomTitle))
        );
    }

    #[test]
    fn a_custom_title_file_wins_when_no_record_is_present() {
        let latches = latches(None, Some("ai"), Some("agent"), Some("prompt"));
        assert_eq!(
            resolve_title(&latches, Some("file wins"), Some("first message")),
            Some(("file wins".to_owned(), TitleSource::CustomTitleFile))
        );
    }

    #[test]
    fn an_ai_title_wins_when_no_custom_title_exists_either_way() {
        let latches = latches(None, Some("ai wins"), Some("agent"), Some("prompt"));
        assert_eq!(resolve_title(&latches, None, Some("first message")), Some(("ai wins".to_owned(), TitleSource::AiTitle)));
    }

    #[test]
    fn an_agent_name_wins_when_no_title_was_ever_generated() {
        let latches = latches(None, None, Some("agent wins"), Some("prompt"));
        assert_eq!(resolve_title(&latches, None, Some("first message")), Some(("agent wins".to_owned(), TitleSource::AgentName)));
    }

    #[test]
    fn the_last_prompt_wins_when_nothing_named_the_session() {
        let latches = latches(None, None, None, Some("prompt wins"));
        assert_eq!(
            resolve_title(&latches, None, Some("first message")),
            Some(("prompt wins".to_owned(), TitleSource::LastPrompt))
        );
    }

    #[test]
    fn the_first_human_message_is_the_last_resort() {
        let latches = latches(None, None, None, None);
        assert_eq!(
            resolve_title(&latches, None, Some("first message wins")),
            Some(("first message wins".to_owned(), TitleSource::FirstMessage))
        );
    }

    #[test]
    fn a_session_with_nothing_at_all_resolves_to_none() {
        let latches = latches(None, None, None, None);
        assert_eq!(resolve_title(&latches, None, None), None);
    }

    #[test]
    fn a_tool_result_user_record_is_not_a_human_turn() {
        let line = br#"{"type":"user","toolUseResult":{"stdout":"x"},"message":{"role":"user","content":[]}}"#;
        assert!(!is_human_turn(&scan_line(line)));
    }

    #[test]
    fn a_meta_user_record_is_not_a_human_turn() {
        let line = br#"{"type":"user","isMeta":true,"message":{"role":"user","content":"x"}}"#;
        assert!(!is_human_turn(&scan_line(line)));
    }

    #[test]
    fn a_non_human_origin_is_not_a_human_turn() {
        let line = br#"{"type":"user","origin":{"kind":"peer"},"message":{"role":"user","content":"x"}}"#;
        assert!(!is_human_turn(&scan_line(line)));
    }

    #[test]
    fn a_missing_origin_is_treated_as_human() {
        let line = br#"{"type":"user","message":{"role":"user","content":"x"}}"#;
        assert!(is_human_turn(&scan_line(line)));
    }

    #[test]
    fn the_first_human_message_reads_a_plain_string_content() {
        let line = br#"{"type":"user","message":{"role":"user","content":"hello there"}}"#;
        assert_eq!(human_message_text(&scan_line(line)), Some("hello there"));
    }

    #[test]
    fn array_content_is_not_read_as_a_title_candidate() {
        let line = br#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#;
        assert_eq!(human_message_text(&scan_line(line)), None);
    }
}
