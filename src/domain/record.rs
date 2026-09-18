//! The load tier: one concrete struct per record shape, dispatched on the type the metadata
//! scanner already read.
//!
//! Never a top-level tagged enum — `type` is not the first key, and tagging would buffer the
//! whole `message` value just to read it.

use jiff::Timestamp;
use serde::{Deserialize, Deserializer};
use thiserror::Error;

use crate::domain::block::{Block, Content};
use crate::domain::latch::{self, Latch};
use crate::domain::scan;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("line has no top-level type")]
    NoType,
    #[error("unknown record type {0:?}")]
    UnknownType(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub enum Record {
    User(Box<UserRecord>),
    Assistant(Box<AssistantRecord>),
    System(Box<SystemRecord>),
    Attachment(Box<AttachmentRecord>),
    Summary(SummaryRecord),
    Latch(Latch),
}

impl Record {
    pub const fn record_type(&self) -> &'static str {
        match self {
            Self::User(_) => "user",
            Self::Assistant(_) => "assistant",
            Self::System(_) => "system",
            Self::Attachment(_) => "attachment",
            Self::Summary(_) => "summary",
            Self::Latch(_) => "latch",
        }
    }

    pub fn unknown_block_kinds(&self) -> Vec<&str> {
        let content = match self {
            Self::User(user) => Some(&user.message.content),
            Self::Assistant(assistant) => Some(&assistant.message.content),
            Self::System(_) | Self::Attachment(_) | Self::Summary(_) | Self::Latch(_) => None,
        };
        content.map_or_else(Vec::new, unknown_block_kinds)
    }
}

fn unknown_block_kinds(content: &Content) -> Vec<&str> {
    match content {
        Content::Text(_) => Vec::new(),
        Content::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                Block::Other { kind } => Some(kind.as_str()),
                Block::Text { .. }
                | Block::Thinking { .. }
                | Block::ToolUse { .. }
                | Block::ToolResult { .. }
                | Block::Image { .. } => None,
            })
            .collect(),
    }
}

const CONCRETE_TYPES: [&str; 12] = [
    "user",
    "assistant",
    "system",
    "attachment",
    "summary",
    "custom-title",
    "ai-title",
    "agent-name",
    "last-prompt",
    "cost-state",
    "continued-in",
    "fork-context-ref",
];

pub fn is_known_type(kind: &str) -> bool {
    CONCRETE_TYPES.contains(&kind) || latch::is_known_latch(kind)
}

pub fn parse(line: &[u8]) -> Result<Record, ParseError> {
    let kind = scan::top_level_str(line, "type").ok_or(ParseError::NoType)?;
    match kind {
        "user" => Ok(Record::User(Box::new(serde_json::from_slice(line)?))),
        "assistant" => Ok(Record::Assistant(Box::new(serde_json::from_slice(line)?))),
        "system" => Ok(Record::System(Box::new(serde_json::from_slice(line)?))),
        "attachment" => Ok(Record::Attachment(Box::new(serde_json::from_slice(line)?))),
        "summary" => Ok(Record::Summary(serde_json::from_slice(line)?)),
        "custom-title" => Ok(Record::Latch(Latch::CustomTitle(serde_json::from_slice(line)?))),
        "ai-title" => Ok(Record::Latch(Latch::AiTitle(serde_json::from_slice(line)?))),
        "agent-name" => Ok(Record::Latch(Latch::AgentName(serde_json::from_slice(line)?))),
        "last-prompt" => Ok(Record::Latch(Latch::LastPrompt(serde_json::from_slice(line)?))),
        "cost-state" => Ok(Record::Latch(Latch::CostState(serde_json::from_slice(line)?))),
        "continued-in" => Ok(Record::Latch(Latch::ContinuedIn(serde_json::from_slice(line)?))),
        "fork-context-ref" => Ok(Record::Latch(Latch::ForkContextRef(serde_json::from_slice(line)?))),
        other if latch::is_known_latch(other) => Ok(Record::Latch(latch::parse_generic(other, line)?)),
        other => Err(ParseError::UnknownType(other.to_owned())),
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    pub uuid: String,
    pub parent_uuid: Option<String>,
    #[serde(default, deserialize_with = "lenient_timestamp")]
    pub timestamp: Option<Timestamp>,
    pub session_id: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub is_sidechain: bool,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub user_type: Option<String>,
    #[serde(default)]
    pub entrypoint: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageBody {
    pub role: String,
    pub content: Content,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub role: String,
    pub content: Content,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub usage: Option<crate::domain::block::Usage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Origin {
    pub kind: String,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub from_mode: Option<String>,
    #[serde(default, rename = "msg_id")]
    pub msg_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub verified_peer_pid: Option<u64>,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRecord {
    #[serde(flatten)]
    pub envelope: Envelope,
    pub message: MessageBody,
    #[serde(default)]
    pub tool_use_result: Option<serde_json::Value>,
    #[serde(default)]
    pub is_meta: bool,
    #[serde(default)]
    pub origin: Option<Origin>,
    #[serde(default)]
    pub prompt_id: Option<String>,
    #[serde(default)]
    pub prompt_source: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default, rename = "sourceToolAssistantUUID")]
    pub source_tool_assistant_uuid: Option<String>,
    #[serde(default)]
    pub is_compact_summary: bool,
    #[serde(default)]
    pub interrupted_message_id: Option<String>,
    #[serde(default)]
    pub tool_denial_kind: Option<String>,
}

impl UserRecord {
    pub fn is_human_turn(&self) -> bool {
        if self.tool_use_result.is_some() || self.is_meta {
            return false;
        }
        self.origin.as_ref().is_none_or(|origin| origin.kind == "human")
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantRecord {
    #[serde(flatten)]
    pub envelope: Envelope,
    pub message: AssistantMessage,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub api_block_index: Option<u32>,
    #[serde(default)]
    pub is_api_error_message: bool,
    #[serde(default)]
    pub attribution_agent: Option<String>,
    #[serde(default)]
    pub attribution_plugin: Option<String>,
    #[serde(default)]
    pub attribution_skill: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactMetadata {
    #[serde(default)]
    pub trigger: Option<String>,
    #[serde(default)]
    pub pre_tokens: Option<u64>,
    #[serde(default)]
    pub post_tokens: Option<u64>,
    #[serde(default)]
    pub cumulative_dropped_tokens: Option<u64>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub pre_compact_discovered_tools: Vec<String>,
    #[serde(default)]
    pub preserved_messages: Option<PreservedMessages>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreservedMessages {
    #[serde(default)]
    pub anchor_uuid: Option<String>,
    #[serde(default)]
    pub uuids: Vec<String>,
    #[serde(default)]
    pub all_uuids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemRecord {
    #[serde(flatten)]
    pub envelope: Envelope,
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub logical_parent_uuid: Option<String>,
    #[serde(default)]
    pub compact_metadata: Option<CompactMetadata>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub is_meta: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRecord {
    #[serde(flatten)]
    pub envelope: Envelope,
    pub attachment: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SummaryRecord {
    pub summary: String,
    #[serde(default, rename = "leafUuid")]
    pub leaf_uuid: Option<String>,
}

pub(crate) fn lenient_timestamp<'de, D>(deserializer: D) -> Result<Option<Timestamp>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: Option<String> = Option::deserialize(deserializer)?;
    Ok(raw.and_then(|text| text.parse::<Timestamp>().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_record_parses_with_its_envelope() {
        let line = br#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#;
        let record = parse(line).expect("a user record");
        let Record::User(user) = record else { panic!("expected a user record") };
        assert_eq!(user.envelope.uuid, "u1");
        assert!(user.envelope.parent_uuid.is_none());
        assert!(user.envelope.timestamp.is_some());
    }

    #[test]
    fn an_assistant_record_takes_the_highest_apiblockindex_fragment_on_its_own() {
        let line = br#"{"parentUuid":"u1","isSidechain":false,"message":{"model":"m","id":"msg_1","role":"assistant","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":1,"output_tokens":2}},"apiBlockIndex":2,"requestId":"r1","type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#;
        let record = parse(line).expect("an assistant record");
        let Record::Assistant(assistant) = record else { panic!("expected an assistant record") };
        assert_eq!(assistant.api_block_index, Some(2));
        assert_eq!(assistant.message.usage.as_ref().map(|usage| usage.output_tokens), Some(2));
    }

    #[test]
    fn a_peer_origin_carries_its_extra_fields() {
        let line = br#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"peer","from":"uds:/tmp/x.sock","fromMode":"prompting","msg_id":"m1","name":"n","verifiedPeerPid":123,"body":"b"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#;
        let record = parse(line).expect("a user record");
        let Record::User(user) = record else { panic!("expected a user record") };
        let origin = user.origin.expect("a peer origin");
        assert_eq!(origin.kind, "peer");
        assert_eq!(origin.msg_id.as_deref(), Some("m1"));
        assert_eq!(origin.verified_peer_pid, Some(123));
    }

    #[test]
    fn a_compact_boundary_carries_its_logical_parent() {
        let line = br#"{"parentUuid":null,"isSidechain":false,"logicalParentUuid":"a1","type":"system","subtype":"compact_boundary","content":"Conversation compacted","uuid":"s1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#;
        let record = parse(line).expect("a system record");
        let Record::System(system) = record else { panic!("expected a system record") };
        assert_eq!(system.subtype.as_deref(), Some("compact_boundary"));
        assert_eq!(system.logical_parent_uuid.as_deref(), Some("a1"));
        assert!(system.envelope.parent_uuid.is_none());
    }

    #[test]
    fn an_attachment_record_keeps_its_payload_as_a_value() {
        let line = br#"{"parentUuid":"u1","isSidechain":false,"type":"attachment","attachment":{"type":"date","date":"2026-01-05"},"uuid":"a1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#;
        let record = parse(line).expect("an attachment record");
        let Record::Attachment(attachment) = record else { panic!("expected an attachment record") };
        assert_eq!(attachment.attachment.get("type").and_then(serde_json::Value::as_str), Some("date"));
    }

    #[test]
    fn a_summary_record_has_no_envelope() {
        let line = br#"{"type":"summary","summary":"Legacy summary","leafUuid":"a1"}"#;
        let record = parse(line).expect("a summary record");
        let Record::Summary(summary) = record else { panic!("expected a summary record") };
        assert_eq!(summary.summary, "Legacy summary");
        assert_eq!(summary.leaf_uuid.as_deref(), Some("a1"));
    }

    #[test]
    fn a_custom_title_record_parses_as_a_dedicated_latch() {
        let line = br#"{"type":"custom-title","customTitle":"Holodeck","sessionId":"s1"}"#;
        let record = parse(line).expect("a latch record");
        assert!(matches!(record, Record::Latch(Latch::CustomTitle(_))));
    }

    #[test]
    fn a_known_but_unmodelled_latch_never_becomes_drift() {
        for kind in ["frame-link", "artifact-autoreact-ledger", "artifact-comment-monitor"] {
            let line = format!(r#"{{"type":"{kind}","sessionId":"s1"}}"#);
            let record = parse(line.as_bytes()).unwrap_or_else(|error| panic!("{kind} must parse as a known latch: {error}"));
            assert!(matches!(record, Record::Latch(Latch::Known { .. })), "{kind} did not parse as a known latch");
        }
    }

    #[test]
    fn a_fork_context_ref_parses_as_its_own_latch_rather_than_the_generic_one() {
        let line = br#"{"type":"fork-context-ref","agentId":"b2c3d4e5f60718293","parentLastUuid":"u9","contextLength":757}"#;
        let record = parse(line).expect("a fork-context-ref");
        let Record::Latch(Latch::ForkContextRef(latch)) = record else { panic!("not a fork-context-ref latch") };
        assert_eq!(latch.agent_id.as_deref(), Some("b2c3d4e5f60718293"));
        assert_eq!(latch.parent_last_uuid.as_deref(), Some("u9"), "the generic latch dropped all of this");
    }

    #[test]
    fn a_compact_boundary_carries_the_numbers_its_divider_is_made_of() {
        let line = br#"{"parentUuid":null,"isSidechain":false,"type":"system","subtype":"compact_boundary","level":"info","compactMetadata":{"trigger":"auto","preTokens":948649,"postTokens":124177,"cumulativeDroppedTokens":824472,"durationMs":211043,"preCompactDiscoveredTools":["Bash"],"preservedMessages":{"uuids":["u1","u2"],"allUuids":["u0","u1","u2"]}},"uuid":"s1","timestamp":"2026-01-01T00:00:00Z","sessionId":"x"}"#;
        let record = parse(line).expect("a compact boundary");
        let Record::System(system) = record else { panic!("not a system record") };
        let meta = system.compact_metadata.expect("compact metadata");
        assert_eq!(meta.trigger.as_deref(), Some("auto"));
        assert_eq!((meta.pre_tokens, meta.post_tokens), (Some(948_649), Some(124_177)));
        assert_eq!(meta.cumulative_dropped_tokens, Some(824_472));
        assert_eq!(meta.duration_ms, Some(211_043));
        let preserved = meta.preserved_messages.expect("preserved messages");
        assert_eq!((preserved.uuids.len(), preserved.all_uuids.len()), (2, 3), "which messages survived, for #15");
    }

    #[test]
    fn every_field_of_compact_metadata_is_optional_so_a_sparser_boundary_is_not_drift() {
        let line = br#"{"parentUuid":null,"isSidechain":false,"type":"system","subtype":"compact_boundary","compactMetadata":{},"uuid":"s1","timestamp":"2026-01-01T00:00:00Z","sessionId":"x"}"#;
        let record = parse(line).expect("a bare compact boundary");
        let Record::System(system) = record else { panic!("not a system record") };
        let meta = system.compact_metadata.expect("compact metadata");
        assert_eq!(meta.pre_tokens, None);
        assert!(meta.pre_compact_discovered_tools.is_empty());
    }

    #[test]
    fn an_unrecognized_type_is_an_unknown_type_error() {
        let line = br#"{"type":"telemetry-latch","sessionId":"s1"}"#;
        let error = parse(line).expect_err("telemetry-latch is not a known type");
        assert!(matches!(error, ParseError::UnknownType(kind) if kind == "telemetry-latch"));
    }

    #[test]
    fn the_metadata_tier_and_the_load_tier_agree_on_what_is_known() {
        for kind in CONCRETE_TYPES {
            assert!(is_known_type(kind), "{kind} parses here but reads as drift at the metadata tier");
            let line = format!(r#"{{"type":"{kind}"}}"#);
            assert!(
                !matches!(parse(line.as_bytes()), Err(ParseError::UnknownType(_))),
                "{kind} is known at the metadata tier but reads as drift here"
            );
        }
        assert!(is_known_type("queue-operation"), "the generic latches are known too");
        assert!(!is_known_type("telemetry-latch"));
    }

    #[test]
    fn a_line_with_no_type_key_is_reported_as_such() {
        let error = parse(br#"{"uuid":"u1"}"#).expect_err("a line with no type must fail");
        assert!(matches!(error, ParseError::NoType));
    }

    #[test]
    fn an_unknown_block_inside_an_assistant_message_is_counted() {
        let line = br#"{"parentUuid":"u1","isSidechain":false,"message":{"role":"assistant","content":[{"type":"text","text":"hi"},{"type":"server_tool_use","id":"x","name":"web_search"}]},"type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#;
        let record = parse(line).expect("an assistant record");
        assert_eq!(record.unknown_block_kinds(), ["server_tool_use"]);
    }

    #[test]
    fn a_bad_timestamp_yields_none_rather_than_failing_the_record() {
        let line = br#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","uuid":"u1","timestamp":"not-a-timestamp","sessionId":"s1"}"#;
        let record = parse(line).expect("a malformed timestamp must not fail the record");
        let Record::User(user) = record else { panic!("expected a user record") };
        assert!(user.envelope.timestamp.is_none());
    }
}
