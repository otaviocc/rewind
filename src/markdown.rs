//! `pulldown-cmark` events become a block tree. Parsing only: nothing here draws.

use std::iter::Peekable;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading { level: u8, inlines: Vec<Inline> },
    Paragraph(Vec<Inline>),
    Quote(Vec<Self>),
    List { ordered: Option<u64>, items: Vec<ListItem> },
    CodeBlock { lang: Option<String>, text: String },
    Table { header: Vec<Vec<Inline>>, rows: Vec<Vec<Vec<Inline>>>, alignments: Vec<Alignment> },
    Rule,
    Html(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    pub blocks: Vec<Block>,
}

impl ListItem {
    pub fn task(&self) -> Option<bool> {
        match self.blocks.first()? {
            Block::Paragraph(inlines) => match inlines.first()? {
                Inline::TaskMarker(checked) => Some(*checked),
                _ => None,
            },
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    None,
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Emphasis(Vec<Self>),
    Strong(Vec<Self>),
    Strike(Vec<Self>),
    Code(String),
    Link { url: String, inlines: Vec<Self> },
    Image { alt: String, url: String },
    Html(String),
    SoftBreak,
    HardBreak,
    TaskMarker(bool),
}

pub fn parse(source: &str) -> Vec<Block> {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut ast = Ast { events: Parser::new_ext(source, options).peekable() };
    ast.blocks()
}

pub fn plain_text(inlines: &[Inline]) -> String {
    let mut text = String::new();
    push_plain_text(inlines, &mut text);
    text
}

fn push_plain_text(inlines: &[Inline], text: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(value) | Inline::Code(value) | Inline::Html(value) => text.push_str(value),
            Inline::Emphasis(children) | Inline::Strong(children) | Inline::Strike(children) => {
                push_plain_text(children, text);
            }
            Inline::Link { inlines, .. } => push_plain_text(inlines, text),
            Inline::Image { alt, .. } => text.push_str(alt),
            Inline::SoftBreak | Inline::HardBreak => text.push(' '),
            Inline::TaskMarker(_) => {}
        }
    }
}

struct Ast<'a, I: Iterator<Item = Event<'a>>> {
    events: Peekable<I>,
}

impl<'a, I: Iterator<Item = Event<'a>>> Ast<'a, I> {
    fn blocks(&mut self) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut loose = Vec::new();

        while let Some(event) = self.events.next_if(|event| !matches!(event, Event::End(_))) {
            if opens_block(&event) {
                if !loose.is_empty() {
                    blocks.push(Block::Paragraph(std::mem::take(&mut loose)));
                }
                if let Some(block) = self.block(event) {
                    blocks.push(block);
                }
            } else if let Some(inline) = self.inline(event) {
                loose.push(inline);
            }
        }

        if !loose.is_empty() {
            blocks.push(Block::Paragraph(loose));
        }
        blocks
    }

    fn block(&mut self, event: Event<'a>) -> Option<Block> {
        let tag = match event {
            Event::Rule => return Some(Block::Rule),
            Event::Start(tag) => tag,
            _ => return None,
        };

        Some(match tag {
            Tag::Paragraph => Block::Paragraph(self.inlines()),
            Tag::Heading { level, .. } => Block::Heading { level: heading_level(level), inlines: self.inlines() },
            Tag::BlockQuote(_) => {
                let blocks = self.blocks();
                self.skip_end();
                Block::Quote(blocks)
            }
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().map(str::to_owned),
                    CodeBlockKind::Indented => None,
                };
                let mut text = String::new();
                for event in self.events.by_ref() {
                    match event {
                        Event::End(_) => break,
                        Event::Text(value) => text.push_str(&value),
                        _ => {}
                    }
                }
                Block::CodeBlock { lang, text }
            }
            Tag::HtmlBlock => {
                let mut text = String::new();
                for event in self.events.by_ref() {
                    match event {
                        Event::End(_) => break,
                        Event::Html(value) | Event::Text(value) => text.push_str(&value),
                        _ => {}
                    }
                }
                Block::Html(text.trim_end_matches('\n').to_owned())
            }
            Tag::List(ordered) => {
                let mut items = Vec::new();
                while let Some(event) = self.events.next() {
                    match event {
                        Event::End(_) => break,
                        Event::Start(Tag::Item) => {
                            let blocks = self.blocks();
                            self.skip_end();
                            items.push(ListItem { blocks });
                        }
                        _ => {}
                    }
                }
                Block::List { ordered, items }
            }
            Tag::Table(alignments) => {
                let mut header = Vec::new();
                let mut rows = Vec::new();
                while let Some(event) = self.events.next() {
                    match event {
                        Event::End(_) => break,
                        Event::Start(Tag::TableHead) => header = self.cells(),
                        Event::Start(Tag::TableRow) => rows.push(self.cells()),
                        _ => {}
                    }
                }
                Block::Table { header, rows, alignments: alignments.into_iter().map(alignment).collect() }
            }
            _ => {
                self.skip_to_end();
                return None;
            }
        })
    }

    fn cells(&mut self) -> Vec<Vec<Inline>> {
        let mut cells = Vec::new();
        while let Some(event) = self.events.next() {
            match event {
                Event::End(_) => break,
                Event::Start(Tag::TableCell) => cells.push(self.inlines()),
                _ => {}
            }
        }
        cells
    }

    fn inlines(&mut self) -> Vec<Inline> {
        let mut inlines = Vec::new();
        while let Some(event) = self.events.next() {
            match event {
                Event::End(_) => break,
                other => {
                    if let Some(inline) = self.inline(other) {
                        inlines.push(inline);
                    }
                }
            }
        }
        inlines
    }

    fn inline(&mut self, event: Event<'a>) -> Option<Inline> {
        Some(match event {
            Event::Text(value) => Inline::Text(value.into_string()),
            Event::Code(value) => Inline::Code(value.into_string()),
            Event::Html(value) | Event::InlineHtml(value) => Inline::Html(value.into_string()),
            Event::SoftBreak => Inline::SoftBreak,
            Event::HardBreak => Inline::HardBreak,
            Event::TaskListMarker(checked) => Inline::TaskMarker(checked),
            Event::Start(Tag::Emphasis) => Inline::Emphasis(self.inlines()),
            Event::Start(Tag::Strong) => Inline::Strong(self.inlines()),
            Event::Start(Tag::Strikethrough) => Inline::Strike(self.inlines()),
            Event::Start(Tag::Link { dest_url, .. }) => Inline::Link { url: dest_url.into_string(), inlines: self.inlines() },
            Event::Start(Tag::Image { dest_url, .. }) => {
                Inline::Image { alt: plain_text(&self.inlines()), url: dest_url.into_string() }
            }
            Event::Start(_) => {
                self.skip_to_end();
                return None;
            }
            _ => return None,
        })
    }

    fn skip_end(&mut self) {
        self.events.next_if(|event| matches!(event, Event::End(_)));
    }

    fn skip_to_end(&mut self) {
        let mut depth = 1usize;
        for event in self.events.by_ref() {
            match event {
                Event::Start(_) => depth = depth.saturating_add(1),
                Event::End(_) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
    }
}

const fn opens_block(event: &Event<'_>) -> bool {
    match event {
        Event::Rule => true,
        Event::Start(tag) => matches!(
            tag,
            Tag::Paragraph
                | Tag::Heading { .. }
                | Tag::BlockQuote(_)
                | Tag::CodeBlock(_)
                | Tag::HtmlBlock
                | Tag::List(_)
                | Tag::Table(_)
        ),
        _ => false,
    }
}

const fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

const fn alignment(alignment: pulldown_cmark::Alignment) -> Alignment {
    match alignment {
        pulldown_cmark::Alignment::None => Alignment::None,
        pulldown_cmark::Alignment::Left => Alignment::Left,
        pulldown_cmark::Alignment::Center => Alignment::Center,
        pulldown_cmark::Alignment::Right => Alignment::Right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(source: &str) -> Block {
        let mut blocks = parse(source);
        assert_eq!(blocks.len(), 1, "expected one block from {source:?}, got {blocks:?}");
        blocks.remove(0)
    }

    fn text(value: &str) -> Inline {
        Inline::Text(value.to_owned())
    }

    #[test]
    fn a_heading_carries_its_level_and_its_inlines() {
        assert_eq!(one("### Deflector"), Block::Heading { level: 3, inlines: vec![text("Deflector")] });
    }

    #[test]
    fn emphasis_strong_and_strikethrough_nest_rather_than_flatten() {
        let Block::Paragraph(inlines) = one("*a **b** ~~c~~*") else { panic!("expected a paragraph") };
        assert_eq!(
            inlines,
            vec![Inline::Emphasis(
                vec![text("a "), Inline::Strong(vec![text("b")]), text(" "), Inline::Strike(vec![text("c")]),]
            )]
        );
    }

    #[test]
    fn a_fenced_block_keeps_its_language_and_an_indented_one_has_none() {
        assert_eq!(
            one("```rust\nfn main() {}\n```"),
            Block::CodeBlock { lang: Some("rust".to_owned()), text: "fn main() {}\n".to_owned() }
        );
        assert_eq!(one("    indented\n"), Block::CodeBlock { lang: None, text: "indented\n".to_owned() });
    }

    #[test]
    fn a_fence_info_string_keeps_only_its_first_word_as_the_language() {
        let Block::CodeBlock { lang, .. } = one("```rust,ignore extra\nx\n```") else { panic!("expected a code block") };
        assert_eq!(lang.as_deref(), Some("rust,ignore"));
    }

    #[test]
    fn a_nested_list_is_a_list_inside_an_item_rather_than_a_flat_sequence() {
        let Block::List { ordered, items } = one("- outer\n  - inner\n") else { panic!("expected a list") };
        assert_eq!(ordered, None);
        assert_eq!(items.len(), 1);
        let inner = items.first().and_then(|item| item.blocks.get(1));
        assert!(matches!(inner, Some(Block::List { .. })), "the second block of the item is the nested list");
    }

    #[test]
    fn an_ordered_list_remembers_the_number_it_starts_at() {
        let Block::List { ordered, .. } = one("7. seven\n8. eight\n") else { panic!("expected a list") };
        assert_eq!(ordered, Some(7));
    }

    #[test]
    fn a_task_item_reports_whether_it_is_ticked() {
        let Block::List { items, .. } = one("- [x] done\n- [ ] todo\n") else { panic!("expected a list") };
        assert_eq!(items.first().and_then(ListItem::task), Some(true));
        assert_eq!(items.get(1).and_then(ListItem::task), Some(false));
        assert_eq!(ListItem { blocks: vec![Block::Paragraph(vec![text("plain")])] }.task(), None);
    }

    #[test]
    fn a_table_keeps_its_header_rows_and_alignments_apart() {
        let source = "| a | b |\n| :- | -: |\n| 1 | 2 |\n";
        let Block::Table { header, rows, alignments } = one(source) else { panic!("expected a table") };
        assert_eq!(header, vec![vec![text("a")], vec![text("b")]]);
        assert_eq!(rows, vec![vec![vec![text("1")], vec![text("2")]]]);
        assert_eq!(alignments, vec![Alignment::Left, Alignment::Right]);
    }

    #[test]
    fn a_rule_is_a_block_of_its_own() {
        assert_eq!(one("---\n"), Block::Rule);
    }

    #[test]
    fn a_block_quote_holds_blocks_rather_than_inlines() {
        assert_eq!(one("> quoted\n"), Block::Quote(vec![Block::Paragraph(vec![text("quoted")])]));
    }

    #[test]
    fn a_link_keeps_its_url_and_its_label_separately() {
        let Block::Paragraph(inlines) = one("[label](https://example.invalid)") else { panic!("expected a paragraph") };
        assert_eq!(inlines, vec![Inline::Link { url: "https://example.invalid".to_owned(), inlines: vec![text("label")] }]);
    }

    #[test]
    fn plain_text_flattens_every_inline_for_measuring_a_table_column() {
        let Block::Paragraph(inlines) = one("a *b* `c` [d](e) ![f](g)") else { panic!("expected a paragraph") };
        assert_eq!(plain_text(&inlines), "a b c d f");
    }

    #[test]
    fn prose_with_no_markdown_in_it_is_one_paragraph() {
        assert_eq!(one("just a sentence"), Block::Paragraph(vec![text("just a sentence")]));
    }

    #[test]
    fn an_empty_source_yields_no_blocks_at_all() {
        assert!(parse("").is_empty());
    }
}
