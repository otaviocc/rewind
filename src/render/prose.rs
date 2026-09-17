//! A Markdown block tree laid out as styled lines, at a given column count.

use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthStr;

use crate::markdown::{Alignment, Block, Inline, ListItem, plain_text};
use crate::render::code;
use crate::render::line::{self, RenderedLine, StyledSpan, split_at_width, truncate, wrap_spans};

const BULLETS: [&str; 3] = ["•", "◦", "▪"];
const QUOTE_GUTTER: &str = "┃ ";
const RULE: &str = "─";
const ELLIPSIS: &str = "…";
const CODE_PAD: usize = 1;
const MIN_COLUMN: usize = 3;

const fn heading_style() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

const fn emphasis_style() -> Style {
    Style::new().add_modifier(Modifier::ITALIC)
}

const fn strong_style() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

const fn strike_style() -> Style {
    Style::new().add_modifier(Modifier::CROSSED_OUT)
}

const fn code_style() -> Style {
    Style::new().fg(Color::Cyan)
}

const fn link_style() -> Style {
    Style::new().fg(Color::Blue).add_modifier(Modifier::UNDERLINED)
}

const fn dim_style() -> Style {
    Style::new().fg(Color::DarkGray)
}

pub fn render(blocks: &[Block], width: usize) -> Vec<RenderedLine> {
    let width = width.max(1);
    let mut lines = blocks_to_lines(blocks, Style::default(), width, 0);
    for line in &mut lines {
        clamp(line, width);
    }
    lines
}

fn clamp(line: &mut RenderedLine, width: usize) {
    if line.width() <= width {
        return;
    }

    let budget = width.saturating_sub(ELLIPSIS.width());
    let mut used = 0usize;
    let mut spans: Vec<StyledSpan> = Vec::new();
    let mut cut = Style::default();
    for span in std::mem::take(&mut line.spans) {
        let span_width = span.width();
        if used.saturating_add(span_width) <= budget {
            used = used.saturating_add(span_width);
            spans.push(span);
            continue;
        }
        cut = span.style;
        let (head, _) = split_at_width(&span.text, budget.saturating_sub(used));
        if !head.is_empty() {
            spans.push(StyledSpan::new(head, cut));
        }
        break;
    }
    spans.push(StyledSpan::new(ELLIPSIS, cut));
    line.spans = spans;
}

fn blocks_to_lines(blocks: &[Block], base: Style, width: usize, depth: usize) -> Vec<RenderedLine> {
    let mut lines: Vec<RenderedLine> = Vec::new();
    for block in blocks {
        if !lines.is_empty() {
            lines.push(RenderedLine::blank());
        }
        lines.extend(block_to_lines(block, base, width, depth));
    }
    lines
}

fn block_to_lines(block: &Block, base: Style, width: usize, depth: usize) -> Vec<RenderedLine> {
    match block {
        Block::Paragraph(inlines) => wrap_inlines(inlines, base, width),
        Block::Heading { inlines, .. } => wrap_inlines(inlines, base.patch(heading_style()), width),
        Block::Quote(blocks) => quote_to_lines(blocks, base, width, depth),
        Block::List { ordered, items } => list_to_lines(*ordered, items, base, width, depth),
        Block::CodeBlock { lang, text } => code_to_lines(lang.as_deref(), text, width),
        Block::Table { header, rows, alignments } => table_to_lines(header, rows, alignments, width),
        Block::Rule => vec![one(RULE.repeat(width), dim_style())],
        Block::Html(text) => text.lines().map(|html| one(truncate(html, width), dim_style())).collect(),
    }
}

fn quote_to_lines(blocks: &[Block], base: Style, width: usize, depth: usize) -> Vec<RenderedLine> {
    let gutter = StyledSpan::new(QUOTE_GUTTER, dim_style());
    let inner = width.saturating_sub(gutter.width()).max(1);
    let mut lines = blocks_to_lines(blocks, base, inner, depth);
    for line in &mut lines {
        line.prefix(gutter.clone());
    }
    lines
}

fn list_to_lines(ordered: Option<u64>, items: &[ListItem], base: Style, width: usize, depth: usize) -> Vec<RenderedLine> {
    let mut lines = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let (marker, style) = marker_for(item, ordered, index, depth);
        let indent = marker.width();

        let mut blocks = item.blocks.clone();
        strip_task_marker(&mut blocks);
        let mut item_lines = item_to_lines(&blocks, base, width.saturating_sub(indent).max(1), depth.saturating_add(1));
        if item_lines.is_empty() {
            item_lines.push(RenderedLine::blank());
        }

        for (offset, line) in item_lines.iter_mut().enumerate() {
            let prefix = if offset == 0 {
                StyledSpan::new(marker.clone(), style)
            } else {
                StyledSpan::new(" ".repeat(indent), Style::default())
            };
            line.prefix(prefix);
        }
        lines.extend(item_lines);
    }
    lines
}

fn item_to_lines(blocks: &[Block], base: Style, width: usize, depth: usize) -> Vec<RenderedLine> {
    let mut lines: Vec<RenderedLine> = Vec::new();
    for block in blocks {
        if !lines.is_empty() && !matches!(block, Block::List { .. }) {
            lines.push(RenderedLine::blank());
        }
        lines.extend(block_to_lines(block, base, width, depth));
    }
    lines
}

fn marker_for(item: &ListItem, ordered: Option<u64>, index: usize, depth: usize) -> (String, Style) {
    match (item.task(), ordered) {
        (Some(true), _) => ("☑ ".to_owned(), dim_style()),
        (Some(false), _) => ("☐ ".to_owned(), dim_style()),
        (None, Some(start)) => {
            let number = u64::try_from(index).map_or(start, |index| start.saturating_add(index));
            (format!("{number}. "), dim_style())
        }
        (None, None) => {
            let bullet = index_of(depth, BULLETS.len()).and_then(|index| BULLETS.get(index)).copied().unwrap_or("•");
            (format!("{bullet} "), dim_style())
        }
    }
}

const fn index_of(depth: usize, len: usize) -> Option<usize> {
    depth.checked_rem(len)
}

fn strip_task_marker(blocks: &mut [Block]) {
    let Some(Block::Paragraph(inlines)) = blocks.first_mut() else { return };
    if !matches!(inlines.first(), Some(Inline::TaskMarker(_))) {
        return;
    }
    inlines.remove(0);
    if let Some(Inline::Text(text)) = inlines.first_mut() {
        *text = text.trim_start().to_owned();
    }
}

fn code_to_lines(lang: Option<&str>, text: &str, width: usize) -> Vec<RenderedLine> {
    let mut lines = vec![fence_line(lang, width)];
    let pad = code_pad(width);
    for code in code::highlight(lang, text).iter() {
        lines.push(code_line(code, pad, width));
    }
    lines.push(one(RULE.repeat(width), dim_style()));
    for line in &mut lines {
        line.inset = pad;
    }
    lines
}

const fn code_pad(width: usize) -> usize {
    if width > CODE_PAD.saturating_mul(2).saturating_add(1) { CODE_PAD } else { 0 }
}

fn fence_line(lang: Option<&str>, width: usize) -> RenderedLine {
    let Some(lang) = lang.map(str::trim).filter(|lang| !lang.is_empty()) else {
        return one(RULE.repeat(width), dim_style());
    };
    let head = format!("{lang} ");
    if head.width() >= width {
        return one(truncate(lang, width), dim_style());
    }
    let mut line = RenderedLine::blank();
    line.push(StyledSpan::new(head.clone(), dim_style()));
    line.push(StyledSpan::new(RULE.repeat(width.saturating_sub(head.width())), dim_style()));
    line
}

fn code_line(code: &[StyledSpan], pad: usize, width: usize) -> RenderedLine {
    let room = width.saturating_sub(pad);
    let total = code.iter().map(StyledSpan::width).fold(0, usize::saturating_add);
    let cut = total > room;
    let budget = if cut { room.saturating_sub(ELLIPSIS.width()) } else { room };

    let mut spans: Vec<StyledSpan> = Vec::with_capacity(code.len().saturating_add(2));
    spans.push(StyledSpan::new(" ".repeat(pad), Style::default()));
    let mut used = 0usize;
    for span in code {
        if used >= budget {
            break;
        }
        let (head, tail) = split_at_width(&span.text, budget.saturating_sub(used));
        if !head.is_empty() {
            used = used.saturating_add(head.width());
            spans.push(StyledSpan::new(head, span.style));
        }
        if !tail.is_empty() {
            break;
        }
    }
    if cut {
        spans.push(StyledSpan::new(ELLIPSIS, dim_style()));
    }
    RenderedLine { spans: line::merge(spans), inset: pad }
}

fn table_to_lines(
    header: &[Vec<Inline>],
    rows: &[Vec<Vec<Inline>>],
    alignments: &[Alignment],
    width: usize,
) -> Vec<RenderedLine> {
    let columns = header.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if columns == 0 {
        return Vec::new();
    }
    let widths = column_widths(header, rows, columns, width);
    let align = |column: usize| alignments.get(column).copied().unwrap_or(Alignment::None);

    let mut lines = vec![rule_line("┌", "┬", "┐", &widths)];
    if !header.is_empty() {
        lines.extend(row_to_lines(header, &widths, heading_style(), &align));
        lines.push(rule_line("├", "┼", "┤", &widths));
    }
    for row in rows {
        lines.extend(row_to_lines(row, &widths, Style::default(), &align));
    }
    lines.push(rule_line("└", "┴", "┘", &widths));
    lines
}

fn column_widths(header: &[Vec<Inline>], rows: &[Vec<Vec<Inline>>], columns: usize, width: usize) -> Vec<usize> {
    let mut widths = vec![0usize; columns];
    for row in std::iter::once(header).chain(rows.iter().map(Vec::as_slice)) {
        for (column, cell) in row.iter().enumerate().take(columns) {
            if let Some(slot) = widths.get_mut(column) {
                *slot = (*slot).max(plain_text(cell).width());
            }
        }
    }

    let furniture = columns.saturating_mul(3).saturating_add(1);
    let natural = widths.iter().copied().fold(0, usize::saturating_add);
    let available = width.saturating_sub(furniture);
    if natural <= available {
        return widths;
    }

    let floor = MIN_COLUMN.min(available.checked_div(columns).unwrap_or(0)).max(1);
    let mut shrunk: Vec<usize> = widths
        .iter()
        .map(|column| column.saturating_mul(available).checked_div(natural.max(1)).unwrap_or(0).max(floor))
        .collect();

    let mut slack = available.saturating_sub(shrunk.iter().copied().fold(0, usize::saturating_add));
    for (column, target) in shrunk.iter_mut().enumerate() {
        let room = widths.get(column).copied().unwrap_or(0).saturating_sub(*target).min(slack);
        *target = target.saturating_add(room);
        slack = slack.saturating_sub(room);
    }

    let mut total = shrunk.iter().copied().fold(0, usize::saturating_add);
    while total > available {
        let widest = shrunk.iter().enumerate().filter(|(_, width)| **width > floor).max_by_key(|(_, width)| **width);
        let Some(column) = widest.map(|(column, _)| column) else { break };
        let Some(slot) = shrunk.get_mut(column) else { break };
        *slot = slot.saturating_sub(1);
        total = total.saturating_sub(1);
    }
    shrunk
}

fn row_to_lines(cells: &[Vec<Inline>], widths: &[usize], style: Style, align: &impl Fn(usize) -> Alignment) -> Vec<RenderedLine> {
    let wrapped: Vec<Vec<RenderedLine>> = widths
        .iter()
        .enumerate()
        .map(|(column, width)| wrap_inlines(cells.get(column).map_or(&[], Vec::as_slice), style, *width))
        .collect();

    let height = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);
    (0..height)
        .map(|offset| {
            let mut line = RenderedLine::blank();
            for (column, width) in widths.iter().enumerate() {
                line.push(StyledSpan::new("│ ", dim_style()));
                let cell = wrapped.get(column).and_then(|cell| cell.get(offset)).cloned().unwrap_or_default();
                let (before, after) = pad_split(align(column), width.saturating_sub(cell.width()));
                line.push(StyledSpan::new(" ".repeat(before), style));
                for span in cell.spans {
                    line.push(span);
                }
                line.push(StyledSpan::new(" ".repeat(after), style));
                line.push(StyledSpan::new(" ", dim_style()));
            }
            line.push(StyledSpan::new("│", dim_style()));
            line.spans = line::merge(std::mem::take(&mut line.spans));
            line
        })
        .collect()
}

fn pad_split(alignment: Alignment, padding: usize) -> (usize, usize) {
    match alignment {
        Alignment::Right => (padding, 0),
        Alignment::Center => {
            let before = padding.checked_div(2).unwrap_or(0);
            (before, padding.saturating_sub(before))
        }
        Alignment::None | Alignment::Left => (0, padding),
    }
}

fn rule_line(left: &str, join: &str, right: &str, widths: &[usize]) -> RenderedLine {
    let mut text = String::from(left);
    for (column, width) in widths.iter().enumerate() {
        if column > 0 {
            text.push_str(join);
        }
        text.push_str(&RULE.repeat(width.saturating_add(2)));
    }
    text.push_str(right);
    one(text, dim_style())
}

fn wrap_inlines(inlines: &[Inline], base: Style, width: usize) -> Vec<RenderedLine> {
    let mut spans = Vec::new();
    flatten(inlines, base, &mut spans);
    wrap_spans(&spans, width)
}

fn flatten(inlines: &[Inline], base: Style, out: &mut Vec<StyledSpan>) {
    for inline in inlines {
        match inline {
            Inline::Text(value) => out.push(StyledSpan::new(value.clone(), base)),
            Inline::Code(value) => out.push(StyledSpan::new(value.clone(), base.patch(code_style()))),
            Inline::Html(value) => out.push(StyledSpan::new(value.clone(), base.patch(dim_style()))),
            Inline::Image { alt, .. } => out.push(StyledSpan::new(format!("[image: {alt}]"), base.patch(dim_style()))),
            Inline::SoftBreak => out.push(StyledSpan::new(" ", base)),
            Inline::HardBreak => out.push(StyledSpan::new("\n", base)),
            Inline::Emphasis(children) => flatten(children, base.patch(emphasis_style()), out),
            Inline::Strong(children) => flatten(children, base.patch(strong_style()), out),
            Inline::Strike(children) => flatten(children, base.patch(strike_style()), out),
            Inline::Link { url, inlines } => {
                flatten(inlines, base.patch(link_style()), out);
                if !url.is_empty() && url.trim() != plain_text(inlines).trim() {
                    out.push(StyledSpan::new(format!(" ({url})"), base.patch(dim_style())));
                }
            }
            Inline::TaskMarker(_) => {}
        }
    }
}

fn one(text: impl Into<String>, style: Style) -> RenderedLine {
    let mut line = RenderedLine::blank();
    line.push(StyledSpan::new(text, style));
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::parse;

    fn lines(source: &str, width: usize) -> Vec<String> {
        render(&parse(source), width).iter().map(RenderedLine::text).collect()
    }

    fn styles(source: &str, width: usize) -> Vec<Vec<Style>> {
        render(&parse(source), width).iter().map(|line| line.spans.iter().map(|span| span.style).collect()).collect()
    }

    #[test]
    fn a_heading_is_bold_and_its_hashes_do_not_survive() {
        assert_eq!(lines("## Deflector", 40), vec!["Deflector"]);
        assert_eq!(styles("## Deflector", 40), vec![vec![heading_style()]]);
    }

    #[test]
    fn a_nested_list_indents_each_level_by_the_width_of_its_marker() {
        assert_eq!(lines("- outer\n  - inner\n    - deepest\n", 40), vec!["• outer", "  ◦ inner", "    ▪ deepest"]);
    }

    #[test]
    fn a_fourth_level_of_nesting_cycles_back_to_the_first_bullet() {
        let deep = "- a\n  - b\n    - c\n      - d\n";
        assert_eq!(lines(deep, 40).last().map(String::as_str), Some("      • d"));
    }

    #[test]
    fn an_ordered_list_counts_from_the_number_it_was_given() {
        assert_eq!(lines("7. seven\n8. eight\n", 40), vec!["7. seven", "8. eight"]);
    }

    #[test]
    fn a_task_item_becomes_a_box_and_loses_its_brackets() {
        assert_eq!(lines("- [x] done\n- [ ] todo\n", 40), vec!["☑ done", "☐ todo"]);
    }

    #[test]
    fn a_wrapped_list_item_lines_up_under_its_own_text_rather_than_the_marker() {
        assert_eq!(lines("- alpha beta gamma delta\n", 12), vec!["• alpha beta", "  gamma", "  delta"]);
    }

    #[test]
    fn a_quote_carries_its_gutter_on_every_line_it_produces() {
        let quoted = lines("> alpha beta gamma delta epsilon\n", 20);
        assert!(quoted.iter().all(|line| line.starts_with("┃ ")), "{quoted:?}");
        assert!(quoted.len() > 1, "the quote was meant to wrap");
    }

    #[test]
    fn a_fenced_block_is_framed_by_its_language_and_a_closing_rule() {
        let fenced = lines("```rust\nfn main() {}\n```", 30);
        assert_eq!(fenced.first().map(String::as_str), Some("rust ─────────────────────────"));
        assert_eq!(fenced.get(1).map(String::as_str), Some(" fn main() {}"));
        assert_eq!(fenced.get(2).map(String::as_str), Some("──────────────────────────────"));
    }

    #[test]
    fn a_fence_with_no_language_opens_on_a_plain_rule() {
        let fenced = lines("```\nplain\n```", 10);
        assert_eq!(fenced.first().map(String::as_str), Some("──────────"));
    }

    #[test]
    fn a_code_line_too_long_for_the_column_is_cut_rather_than_wrapped() {
        let fenced = lines("```\nalpha beta gamma delta\n```", 12);
        assert_eq!(fenced.get(1).map(String::as_str), Some(" alpha beta…"));
    }

    #[test]
    fn a_table_fits_the_column_it_was_given_even_when_it_has_to_shrink() {
        let source = "| alpha | beta |\n| --- | --- |\n| one two three | four |\n";
        for width in [12, 20, 40, 80] {
            for line in render(&parse(source), width) {
                assert!(line.width() <= width, "width {width}: {:?}", line.text());
            }
        }
    }

    #[test]
    fn a_table_aligns_each_column_the_way_its_delimiter_row_asked() {
        let source = "| left | centre | right |\n| :--- | :----: | ----: |\n| a | b | c |\n";
        let table = lines(source, 40);
        assert_eq!(table.get(1).map(String::as_str), Some("│ left │ centre │ right │"));
        assert_eq!(table.get(3).map(String::as_str), Some("│ a    │   b    │     c │"));
    }

    #[test]
    fn a_rule_fills_the_column_exactly() {
        assert_eq!(lines("---\n", 8), vec!["────────"]);
    }

    #[test]
    fn a_link_keeps_its_url_beside_the_label_unless_they_are_the_same() {
        assert_eq!(lines("[label](https://host.invalid)", 60), vec!["label (https://host.invalid)"]);
        assert_eq!(lines("<https://host.invalid>", 60), vec!["https://host.invalid"]);
    }

    #[test]
    fn a_hard_break_ends_the_line_where_a_soft_one_would_not() {
        assert_eq!(lines("alpha\nbeta", 40), vec!["alpha beta"]);
        assert_eq!(lines("alpha  \nbeta", 40), vec!["alpha", "beta"]);
    }

    #[test]
    fn inline_code_emphasis_and_strong_each_carry_their_own_style() {
        let styled = styles("`a` *b* **c**", 40).into_iter().flatten().collect::<Vec<_>>();
        assert!(styled.contains(&code_style()));
        assert!(styled.contains(&emphasis_style()));
        assert!(styled.contains(&strong_style()));
    }

    #[test]
    fn every_line_of_a_document_of_every_element_fits_the_column_it_was_wrapped_for() {
        let source = "# h\n\npara *a* `b`\n\n| x | y |\n| - | - |\n| 1 | 2 |\n\n- a\n  - b\n\n- [x] t\n\n> q\n\n---\n\n```rust\nfn main() {}\n```\n";
        for width in 1..=60 {
            for line in render(&parse(source), width) {
                assert!(line.width() <= width, "width {width} overflowed with {:?}", line.text());
            }
        }
    }

    #[test]
    fn nothing_at_all_renders_as_nothing_at_all() {
        assert!(render(&parse(""), 40).is_empty());
    }
}
