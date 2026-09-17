//! Latch and event records: the family that carries only `sessionId` (or, for the
//! file-history pair, only `messageId`) and no transcript envelope at all.

use jiff::Timestamp;
use serde::Deserialize;

use crate::domain::record::lenient_timestamp;

pub const KNOWN_LATCHES: [&str; 12] = [
    "mode",
    "permission-mode",
    "atis-latch",
    "agent-color",
    "pr-link",
    "frame-link",
    "queue-operation",
    "fork-context-ref",
    "file-history-snapshot",
    "file-history-delta",
    "artifact-autoreact-ledger",
    "artifact-comment-monitor",
];

pub fn is_known_latch(kind: &str) -> bool {
    KNOWN_LATCHES.contains(&kind) || kind.starts_with("artifact-")
}

#[derive(Debug, Clone, PartialEq)]
pub enum Latch {
    CustomTitle(CustomTitleLatch),
    AiTitle(AiTitleLatch),
    AgentName(AgentNameLatch),
    LastPrompt(LastPromptLatch),
    CostState(CostStateLatch),
    ContinuedIn(ContinuedInLatch),
    Known { kind: String, session_id: Option<String>, message_id: Option<String>, timestamp: Option<Timestamp> },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CustomTitleLatch {
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    #[serde(rename = "customTitle")]
    pub custom_title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AiTitleLatch {
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    #[serde(rename = "aiTitle")]
    pub ai_title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentNameLatch {
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    #[serde(rename = "agentName")]
    pub agent_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LastPromptLatch {
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    #[serde(rename = "lastPrompt", default)]
    pub last_prompt: Option<String>,
    #[serde(rename = "leafUuid", default)]
    pub leaf_uuid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CostStateLatch {
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    #[serde(rename = "totalCostUSD", default)]
    pub total_cost_usd: Option<f64>,
    #[serde(rename = "totalDuration", default)]
    pub total_duration: Option<u64>,
    #[serde(rename = "modelUsage", default)]
    pub model_usage: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ContinuedInLatch {
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    #[serde(rename = "continuedInSessionId")]
    pub continued_in_session_id: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct GenericLatch {
    #[serde(rename = "sessionId", default)]
    session_id: Option<String>,
    #[serde(rename = "messageId", default)]
    message_id: Option<String>,
    #[serde(default, deserialize_with = "lenient_timestamp")]
    timestamp: Option<Timestamp>,
}

pub fn parse_generic(kind: &str, line: &[u8]) -> Result<Latch, serde_json::Error> {
    let generic: GenericLatch = serde_json::from_slice(line)?;
    Ok(Latch::Known {
        kind: kind.to_owned(),
        session_id: generic.session_id,
        message_id: generic.message_id,
        timestamp: generic.timestamp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_custom_title_latch_deserializes() {
        let latch: CustomTitleLatch =
            serde_json::from_str(r#"{"type":"custom-title","customTitle":"Holodeck","sessionId":"s1"}"#).expect("a latch");
        assert_eq!(latch.custom_title, "Holodeck");
        assert_eq!(latch.session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn a_last_prompt_latch_carries_its_leaf_uuid() {
        let latch: LastPromptLatch =
            serde_json::from_str(r#"{"type":"last-prompt","lastPrompt":"hi","leafUuid":"u1","sessionId":"s1"}"#)
                .expect("a latch");
        assert_eq!(latch.leaf_uuid.as_deref(), Some("u1"));
    }

    #[test]
    fn a_last_prompt_latch_can_be_missing_its_own_prompt() {
        let latch: LastPromptLatch =
            serde_json::from_str(r#"{"type":"last-prompt","leafUuid":"u1","sessionId":"s1"}"#).expect("a latch");
        assert!(latch.last_prompt.is_none());
    }

    #[test]
    fn a_file_history_delta_kind_is_a_known_latch_with_no_session_id_field_required() {
        assert!(is_known_latch("file-history-delta"));
        let latch = parse_generic(
            "file-history-delta",
            br#"{"type":"file-history-delta","messageId":"m1","timestamp":"2026-01-05T09:15:30.000Z"}"#,
        )
        .expect("a generic latch");
        let Latch::Known { session_id, message_id, .. } = latch else { panic!("expected a known latch") };
        assert!(session_id.is_none());
        assert_eq!(message_id.as_deref(), Some("m1"));
    }

    #[test]
    fn frame_link_fork_context_ref_and_artifact_types_are_known_latches() {
        assert!(is_known_latch("frame-link"));
        assert!(is_known_latch("fork-context-ref"));
        assert!(is_known_latch("artifact-autoreact-ledger"));
        assert!(is_known_latch("artifact-comment-monitor"));
        assert!(is_known_latch("artifact-something-not-yet-invented"));
    }

    #[test]
    fn an_unrecognized_kind_is_not_a_known_latch() {
        assert!(!is_known_latch("telemetry-latch"));
    }
}
