//! The element table: every styled thing rewind draws.

use serde::Deserialize;

use ratatui::style::{Color, Modifier, Style};

use crate::theme::color::ColorSpec;
use crate::theme::palette::Palette;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Element {
    Body,
    Muted,
    Label,
    HumanGutter,
    AssistantGutter,
    ToolName,
    ToolSummary,
    ToolOk,
    ToolError,
    Thinking,
    DiffAdded,
    DiffRemoved,
    DiffContext,
    Subagent,
    Injection,
    BranchMarker,
    CompactDivider,
    ProjectMissing,
    SessionLive,
    Heading,
    Strong,
    Emphasis,
    Strikethrough,
    InlineCode,
    CodeBlock,
    CodeBlockLang,
    Link,
    Quote,
    QuoteGutter,
    ListBullet,
    TableHeader,
    TableBorder,
    Rule,
    Html,
    HeaderTitle,
    Status,
    StatusNotice,
    StatusError,
    CursorLine,
    SearchMatch,
    SearchCurrent,
    Selection,
    HelpWindow,
    ScrollProgress,
    Hint,
    ColumnTitle,
    ColumnTitleActive,
}

impl Element {
    pub const ALL: [Self; 47] = [
        Self::Body,
        Self::Muted,
        Self::Label,
        Self::HumanGutter,
        Self::AssistantGutter,
        Self::ToolName,
        Self::ToolSummary,
        Self::ToolOk,
        Self::ToolError,
        Self::Thinking,
        Self::DiffAdded,
        Self::DiffRemoved,
        Self::DiffContext,
        Self::Subagent,
        Self::Injection,
        Self::BranchMarker,
        Self::CompactDivider,
        Self::ProjectMissing,
        Self::SessionLive,
        Self::Heading,
        Self::Strong,
        Self::Emphasis,
        Self::Strikethrough,
        Self::InlineCode,
        Self::CodeBlock,
        Self::CodeBlockLang,
        Self::Link,
        Self::Quote,
        Self::QuoteGutter,
        Self::ListBullet,
        Self::TableHeader,
        Self::TableBorder,
        Self::Rule,
        Self::Html,
        Self::HeaderTitle,
        Self::Status,
        Self::StatusNotice,
        Self::StatusError,
        Self::CursorLine,
        Self::SearchMatch,
        Self::SearchCurrent,
        Self::Selection,
        Self::HelpWindow,
        Self::ScrollProgress,
        Self::Hint,
        Self::ColumnTitle,
        Self::ColumnTitleActive,
    ];

    #[expect(
        clippy::as_conversions,
        reason = "the discriminant is the index, and every_element_is_listed_once_in_discriminant_order proves it"
    )]
    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Body => "body",
            Self::Muted => "muted",
            Self::Label => "label",
            Self::HumanGutter => "human_gutter",
            Self::AssistantGutter => "assistant_gutter",
            Self::ToolName => "tool_name",
            Self::ToolSummary => "tool_summary",
            Self::ToolOk => "tool_ok",
            Self::ToolError => "tool_error",
            Self::Thinking => "thinking",
            Self::DiffAdded => "diff_added",
            Self::DiffRemoved => "diff_removed",
            Self::DiffContext => "diff_context",
            Self::Subagent => "subagent",
            Self::Injection => "injection",
            Self::BranchMarker => "branch_marker",
            Self::CompactDivider => "compact_divider",
            Self::ProjectMissing => "project_missing",
            Self::SessionLive => "session_live",
            Self::Heading => "heading",
            Self::Strong => "strong",
            Self::Emphasis => "emphasis",
            Self::Strikethrough => "strikethrough",
            Self::InlineCode => "inline_code",
            Self::CodeBlock => "code_block",
            Self::CodeBlockLang => "code_block_lang",
            Self::Link => "link",
            Self::Quote => "quote",
            Self::QuoteGutter => "quote_gutter",
            Self::ListBullet => "list_bullet",
            Self::TableHeader => "table_header",
            Self::TableBorder => "table_border",
            Self::Rule => "rule",
            Self::Html => "html",
            Self::HeaderTitle => "header_title",
            Self::Status => "status",
            Self::StatusNotice => "status_notice",
            Self::StatusError => "status_error",
            Self::CursorLine => "cursor_line",
            Self::SearchMatch => "search_match",
            Self::SearchCurrent => "search_current",
            Self::Selection => "selection",
            Self::HelpWindow => "help_window",
            Self::ScrollProgress => "scroll_progress",
            Self::Hint => "hint",
            Self::ColumnTitle => "column_title",
            Self::ColumnTitleActive => "column_title_active",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|element| element.key() == key)
    }
}

pub fn default_style(element: Element, palette: &Palette) -> Style {
    let style = Style::default();
    match element {
        Element::Body => style.fg(palette.foreground),
        Element::Muted
        | Element::Thinking
        | Element::Injection
        | Element::BranchMarker
        | Element::CompactDivider
        | Element::ToolOk
        | Element::ToolSummary
        | Element::ProjectMissing
        | Element::CodeBlockLang
        | Element::QuoteGutter
        | Element::ListBullet
        | Element::TableBorder
        | Element::Rule
        | Element::Html
        | Element::Hint
        | Element::ColumnTitle => style.fg(palette.muted),
        Element::Label | Element::Heading | Element::Strong | Element::TableHeader | Element::Subagent => {
            style.add_modifier(Modifier::BOLD)
        }
        Element::ToolName | Element::HumanGutter | Element::ColumnTitleActive | Element::HeaderTitle => {
            style.fg(palette.accent).add_modifier(Modifier::BOLD)
        }
        Element::AssistantGutter => style.fg(palette.notice).add_modifier(Modifier::DIM),
        Element::DiffContext | Element::CodeBlock => style,
        Element::ToolError | Element::StatusError | Element::DiffRemoved => style.fg(palette.error),
        Element::DiffAdded | Element::SessionLive => style.fg(palette.success),
        Element::Emphasis => style.add_modifier(Modifier::ITALIC),
        Element::Strikethrough => style.add_modifier(Modifier::CROSSED_OUT),
        Element::InlineCode | Element::ScrollProgress => style.fg(palette.accent),
        Element::Link => style.fg(palette.highlight).add_modifier(Modifier::UNDERLINED),
        Element::Quote | Element::Status => style.fg(palette.muted_text),
        Element::StatusNotice => style.fg(palette.warning),
        Element::CursorLine => style.bg(palette.cursor),
        Element::SearchMatch => style.fg(Color::Black).bg(palette.warning),
        Element::SearchCurrent => style.fg(Color::Black).bg(palette.notice).add_modifier(Modifier::BOLD),
        Element::Selection => style.fg(palette.selection_foreground).bg(palette.selection_background),
        Element::HelpWindow => style.fg(palette.foreground).bg(palette.background),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct ElementFile {
    pub fg: Option<ColorSpec>,
    pub bg: Option<ColorSpec>,
    pub modifiers: Option<Vec<String>>,
    #[serde(flatten)]
    pub unknown: std::collections::BTreeMap<String, toml::Value>,
}

impl ElementFile {
    #[must_use]
    pub fn merge(self, base: Self) -> Self {
        let mut unknown = base.unknown;
        unknown.extend(self.unknown);
        Self { fg: self.fg.or(base.fg), bg: self.bg.or(base.bg), modifiers: self.modifiers.or(base.modifiers), unknown }
    }
}

pub fn modifier(name: &str) -> Option<Modifier> {
    Some(match name {
        "bold" => Modifier::BOLD,
        "italic" => Modifier::ITALIC,
        "underline" => Modifier::UNDERLINED,
        "dim" => Modifier::DIM,
        "reversed" => Modifier::REVERSED,
        "crossed_out" => Modifier::CROSSED_OUT,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_element_field_merges_independently_of_the_others() {
        let base: ElementFile = toml::from_str("fg = \"red\"\nmodifiers = [\"bold\"]\n").expect("a base element");
        let child: ElementFile = toml::from_str("bg = \"blue\"\n").expect("a child element");

        let merged = child.merge(base);
        assert_eq!(merged.fg, Some(ColorSpec::Name(String::from("red"))), "the base's foreground was dropped");
        assert_eq!(merged.bg, Some(ColorSpec::Name(String::from("blue"))));
        assert_eq!(merged.modifiers.as_deref(), Some(["bold".to_string()].as_slice()), "the base's modifiers were dropped");

        let base: ElementFile = toml::from_str("fg = \"red\"\n").expect("a base element");
        let child: ElementFile = toml::from_str("fg = \"green\"\nmodifiers = []\n").expect("a child element");
        let merged = child.merge(base);
        assert_eq!(merged.fg, Some(ColorSpec::Name(String::from("green"))), "the child does not win its own field");
        assert_eq!(merged.modifiers, Some(Vec::new()), "an empty list must survive as a way to clear them");
    }

    #[test]
    fn every_element_has_a_key_that_finds_it_again() {
        for element in Element::ALL {
            assert_eq!(Element::from_key(element.key()), Some(element), "{element:?} is not addressable by its key");
        }
    }

    #[test]
    fn keys_are_the_snake_case_names_the_readme_will_document() {
        assert_eq!(Element::InlineCode.key(), "inline_code");
        assert_eq!(Element::CodeBlockLang.key(), "code_block_lang");
        assert_eq!(Element::from_key("code_block_lang"), Some(Element::CodeBlockLang));
        assert_eq!(Element::from_key("headings"), None);
    }

    #[test]
    fn the_documented_modifiers_are_the_ones_accepted() {
        assert_eq!(modifier("bold"), Some(Modifier::BOLD));
        assert_eq!(modifier("underline"), Some(Modifier::UNDERLINED));
        assert_eq!(modifier("crossed_out"), Some(Modifier::CROSSED_OUT));
        assert_eq!(modifier("blinky"), None);
        assert_eq!(modifier("BOLD"), None);
    }
}
