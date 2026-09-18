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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Text { text: String },
    Thinking { thinking: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: Option<String>, content: Option<ToolResultContent>, is_error: Option<bool> },
    Image { source: ImageSource },
    Other { kind: String },
}

#[derive(Default)]
struct BlockFields {
    text: Option<serde_json::Value>,
    thinking: Option<serde_json::Value>,
    id: Option<serde_json::Value>,
    name: Option<serde_json::Value>,
    input: Option<serde_json::Value>,
    tool_use_id: Option<serde_json::Value>,
    content: Option<serde_json::Value>,
    is_error: Option<serde_json::Value>,
    source: Option<serde_json::Value>,
}

fn field<T: serde::de::DeserializeOwned>(value: Option<serde_json::Value>) -> Result<T, serde_json::Error> {
    serde_json::from_value(value.unwrap_or(serde_json::Value::Null))
}

fn assemble(kind: String, fields: BlockFields) -> Result<Block, serde_json::Error> {
    match kind.as_str() {
        "text" => Ok(Block::Text { text: field(fields.text)? }),
        "thinking" => Ok(Block::Thinking { thinking: field(fields.thinking)? }),
        "tool_use" => Ok(Block::ToolUse {
            id: field(fields.id)?,
            name: field(fields.name)?,
            input: fields.input.unwrap_or(serde_json::Value::Null),
        }),
        "tool_result" => Ok(Block::ToolResult {
            tool_use_id: field(fields.tool_use_id)?,
            content: field(fields.content)?,
            is_error: field(fields.is_error)?,
        }),
        "image" => Ok(Block::Image { source: field(fields.source)? }),
        _ => Ok(Block::Other { kind }),
    }
}

impl<'de> Deserialize<'de> for Block {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BlockVisitor;

        impl<'de> Visitor<'de> for BlockVisitor {
            type Value = Block;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a content block object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut kind: Option<String> = None;
                let mut fields = BlockFields::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "type" => kind = Some(map.next_value()?),
                        "text" => fields.text = Some(map.next_value()?),
                        "thinking" => fields.thinking = Some(map.next_value()?),
                        "id" => fields.id = Some(map.next_value()?),
                        "name" => fields.name = Some(map.next_value()?),
                        "input" => fields.input = Some(map.next_value()?),
                        "tool_use_id" => fields.tool_use_id = Some(map.next_value()?),
                        "content" => fields.content = Some(map.next_value()?),
                        "is_error" => fields.is_error = Some(map.next_value()?),
                        "source" => fields.source = Some(map.next_value()?),
                        _ => {
                            let _ignored: de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let kind = kind.ok_or_else(|| de::Error::missing_field("type"))?;
                assemble(kind, fields).map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_map(BlockVisitor)
    }
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
    fn an_unknown_block_keeps_the_name_it_arrived_with() {
        let block: Block = serde_json::from_str(r#"{"type":"server_tool_use","id":"x","name":"web_search"}"#).expect("a block");
        assert_eq!(block, Block::Other { kind: "server_tool_use".to_owned() });
    }

    #[test]
    fn a_block_with_no_type_at_all_is_a_parse_failure_rather_than_an_unknown_block() {
        let result = serde_json::from_str::<Block>(r#"{"text":"hi"}"#);
        assert!(result.is_err());
    }

    #[test]
    fn the_key_order_does_not_decide_the_variant() {
        let block: Block = serde_json::from_str(r#"{"text":"hi","type":"text"}"#).expect("a text block");
        assert_eq!(block, Block::Text { text: "hi".to_owned() });
    }

    #[test]
    fn a_tool_use_block_defaults_its_input_to_null() {
        let block: Block = serde_json::from_str(r#"{"type":"tool_use","id":"t1","name":"Bash"}"#).expect("a tool_use block");
        assert_eq!(block, Block::ToolUse { id: "t1".to_owned(), name: "Bash".to_owned(), input: serde_json::Value::Null });
    }

    #[test]
    fn a_thinking_block_reads_its_own_field() {
        let block: Block =
            serde_json::from_str(r#"{"type":"thinking","thinking":"hmm","signature":"sig"}"#).expect("a thinking block");
        assert_eq!(block, Block::Thinking { thinking: "hmm".to_owned() });
    }

    #[test]
    fn an_unknown_block_carrying_a_field_a_known_block_also_uses_is_still_unknown() {
        let block: Block =
            serde_json::from_str(r#"{"type":"web_search_result","content":[{"title":"x"}]}"#).expect("an unknown block");
        assert_eq!(block, Block::Other { kind: "web_search_result".to_owned() });
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
