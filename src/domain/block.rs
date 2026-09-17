//! The polymorphic pieces inside `message`: content blocks, tool results, the redacted image
//! reference, and `usage`.

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Blocks(Vec<Block>),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type")]
pub enum Block {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        #[serde(default)]
        tool_use_id: Option<String>,
        #[serde(default)]
        content: Option<ToolResultContent>,
        #[serde(default)]
        is_error: Option<bool>,
    },
    #[serde(rename = "image")]
    Image { source: ImageSource },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    Text(String),
    Blocks(Vec<TextBlock>),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TextBlock {
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageSource {
    pub media_type: Option<String>,
    pub bytes: usize,
}

impl<'de> Deserialize<'de> for ImageSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ImageSourceVisitor;

        impl<'de> Visitor<'de> for ImageSourceVisitor {
            type Value = ImageSource;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an image source object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut media_type = None;
                let mut bytes = 0usize;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "data" => {
                            let data: String = map.next_value()?;
                            if !data.is_empty() {
                                bytes = data.len();
                            }
                        }
                        "redactedBytes" => bytes = map.next_value()?,
                        "media_type" => media_type = Some(map.next_value()?),
                        _ => {
                            let _ignored: de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(ImageSource { media_type, bytes })
            }
        }

        deserializer.deserialize_map(ImageSourceVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub output_tokens_details: Option<OutputTokensDetails>,
    #[serde(default)]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub inference_geo: Option<String>,
    #[serde(default)]
    pub speed: Option<String>,
    #[serde(default)]
    pub cache_creation: Option<serde_json::Value>,
    #[serde(default)]
    pub server_tool_use: Option<serde_json::Value>,
    #[serde(default)]
    pub iterations: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub struct OutputTokensDetails {
    #[serde(default)]
    pub thinking_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_string_content_is_the_text_variant() {
        let content: Content = serde_json::from_str(r#""hello""#).expect("a string content");
        assert_eq!(content, Content::Text("hello".to_owned()));
    }

    #[test]
    fn an_array_content_is_the_blocks_variant() {
        let content: Content = serde_json::from_str(r#"[{"type":"text","text":"hi"}]"#).expect("a blocks content");
        assert_eq!(content, Content::Blocks(vec![Block::Text { text: "hi".to_owned() }]));
    }

    #[test]
    fn an_unknown_block_type_is_the_other_variant() {
        let block: Block = serde_json::from_str(r#"{"type":"server_tool_use","id":"x","name":"web_search"}"#).expect("a block");
        assert_eq!(block, Block::Other);
    }

    #[test]
    fn an_image_source_never_materializes_the_payload() {
        let source: ImageSource =
            serde_json::from_str(r#"{"type":"base64","media_type":"image/png","data":"abcde"}"#).expect("an image source");
        assert_eq!(source.media_type.as_deref(), Some("image/png"));
        assert_eq!(source.bytes, 5);
    }

    #[test]
    fn a_redacted_image_source_reports_the_length_the_reader_elided() {
        let source: ImageSource =
            serde_json::from_str(r#"{"type":"base64","media_type":"image/png","data":"","redactedBytes":1960000}"#)
                .expect("a redacted image source");
        assert_eq!(source.media_type.as_deref(), Some("image/png"));
        assert_eq!(source.bytes, 1_960_000);
    }

    #[test]
    fn a_tool_result_content_can_be_a_bare_string() {
        let block: Block = serde_json::from_str(r#"{"type":"tool_result","tool_use_id":"t1","content":"ok","is_error":false}"#)
            .expect("a tool_result block");
        let Block::ToolResult { content, .. } = block else { panic!("expected a tool_result block") };
        assert_eq!(content, Some(ToolResultContent::Text("ok".to_owned())));
    }

    #[test]
    fn a_tool_result_content_can_be_an_array_of_text_blocks() {
        let block: Block = serde_json::from_str(r#"{"type":"tool_result","content":[{"type":"text","text":"ok"}]}"#)
            .expect("a tool_result block");
        let Block::ToolResult { content, .. } = block else { panic!("expected a tool_result block") };
        assert_eq!(content, Some(ToolResultContent::Blocks(vec![TextBlock { text: "ok".to_owned() }])));
    }

    #[test]
    fn the_legacy_usage_shape_deserializes_with_defaults() {
        let usage: Usage = serde_json::from_str(
            r#"{"input_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":8100,"output_tokens":34}"#,
        )
        .expect("a legacy usage shape");
        assert_eq!(usage.input_tokens, 2);
        assert_eq!(usage.output_tokens, 34);
        assert!(usage.output_tokens_details.is_none());
    }

    #[test]
    fn a_synthetic_error_usage_nulls_service_tier_and_output_tokens_details() {
        let usage: Usage = serde_json::from_str(
            r#"{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens_details":null,"service_tier":null}"#,
        )
        .expect("a synthetic-error usage shape");
        assert!(usage.output_tokens_details.is_none());
        assert!(usage.service_tier.is_none());
    }

    #[test]
    fn the_rich_usage_shape_reads_thinking_tokens() {
        let usage: Usage = serde_json::from_str(
            r#"{"input_tokens":2,"cache_creation_input_tokens":17453,"cache_read_input_tokens":23618,"output_tokens":245,"output_tokens_details":{"thinking_tokens":115}}"#,
        )
        .expect("a rich usage shape");
        assert_eq!(usage.output_tokens_details, Some(OutputTokensDetails { thinking_tokens: 115 }));
    }
}
