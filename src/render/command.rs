//! A local slash command's two record shapes as a compact block.
//!
//! The invocation renders as a name row with its args dimmed beneath it, and the stdout result
//! as a dimmed row of its own — reusing the two-row head, gutter and fold machinery
//! `render/tool.rs` built for tool calls, since the shape is the same and there is no outcome
//! column to draw.

use unicode_width::UnicodeWidthStr;

use crate::render::Ctx;
use crate::render::line::{RenderedLine, StyledSpan, normalise, truncate};
use crate::render::tool::{self, Call, RowStyles};

const OUTPUT_INDENT: &str = "  └ ";

pub(super) fn invocation(id: &str, name: &str, args: Option<&str>, ctx: &Ctx<'_>, styles: &RowStyles) -> Call {
    let expanded = ctx.is_expanded(id);
    let glyph = if expanded { tool::EXPANDED } else { tool::COLLAPSED };
    let digest = args.map(|args| normalise(args).replace('\n', " ")).unwrap_or_default();
    let mut lines = tool::head(glyph, name, &digest, None, None, ctx.width, styles);
    let head = lines.len();
    if let Some(args) = args.filter(|args| expanded && normalise(args).lines().count() > 1) {
        lines.extend(body(ctx.width, args, styles));
    }
    Call { lines, head }
}

pub(super) fn output(id: &str, text: &str, ctx: &Ctx<'_>, styles: &RowStyles) -> Call {
    let normalised = normalise(text);
    let mut source = normalised.lines();
    let first = source.next().unwrap_or_default();
    if source.next().is_none() {
        return Call { lines: vec![row(OUTPUT_INDENT, first, ctx.width, styles)], head: 1 };
    }
    let expanded = ctx.is_expanded(id);
    let glyph = if expanded { tool::EXPANDED } else { tool::COLLAPSED };
    let mut lines = vec![row(glyph, first, ctx.width, styles)];
    if expanded {
        lines.extend(body(ctx.width, text, styles));
    }
    Call { lines, head: 1 }
}

fn body(width: usize, text: &str, styles: &RowStyles) -> Vec<RenderedLine> {
    let inner = width.saturating_sub(tool::GUTTER.width()).max(1);
    let lines: Vec<RenderedLine> = normalise(text).lines().map(|line| tool::cut(line, inner, styles.digest)).collect();
    let mut lines = tool::folded(lines, inner, styles.muted);
    for line in &mut lines {
        tool::detrail(line);
        let glyph = if line.spans.is_empty() { tool::GUTTER_BLANK } else { tool::GUTTER };
        line.prefix(StyledSpan::new(glyph, styles.muted));
    }
    lines
}

fn row(glyph: &str, text: &str, width: usize, styles: &RowStyles) -> RenderedLine {
    let room = width.saturating_sub(glyph.width()).max(1);
    let mut line = RenderedLine::blank();
    line.push(StyledSpan::new(glyph, styles.muted));
    line.push(StyledSpan::new(truncate(text, room), styles.digest));
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::subagent::Agents;
    use crate::render::{Branches, Expanded, Outputs};
    use crate::theme::Theme;
    use ratatui::style::{Color, Modifier, Style};

    fn styles() -> RowStyles {
        RowStyles {
            body: Style::new(),
            name: Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
            digest: Style::new().fg(Color::DarkGray),
            muted: Style::new().fg(Color::DarkGray),
            enter: Style::new(),
        }
    }

    fn ctx(width: usize, expanded: &Expanded) -> Ctx<'_> {
        static THEME: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
        static OUTPUTS: std::sync::OnceLock<Outputs> = std::sync::OnceLock::new();
        static AGENTS: std::sync::OnceLock<Agents> = std::sync::OnceLock::new();
        static BRANCHES: std::sync::OnceLock<Branches> = std::sync::OnceLock::new();
        Ctx {
            width,
            theme: THEME.get_or_init(Theme::default),
            expanded,
            outputs: OUTPUTS.get_or_init(Outputs::new),
            agents: AGENTS.get_or_init(Agents::default),
            root: None,
            branches: BRANCHES.get_or_init(Branches::new),
            injections: false,
        }
    }

    #[test]
    fn a_command_with_no_args_is_one_row() {
        let expanded = Expanded::new();
        let call = invocation("t1", "/plan", None, &ctx(40, &expanded), &styles());
        assert_eq!(call.lines.len(), 1);
        assert_eq!(call.head, 1);
        assert!(call.lines[0].text().starts_with("▸ /plan"), "{:?}", call.lines[0].text());
    }

    #[test]
    fn a_command_with_args_gets_a_dimmed_digest_row() {
        let expanded = Expanded::new();
        let call = invocation("t1", "/opsx:explore", Some("the third view is hard to read"), &ctx(60, &expanded), &styles());
        assert_eq!(call.lines.len(), 2);
        assert_eq!(call.head, 2);
        assert_eq!(call.lines[1].text(), "  └ the third view is hard to read");
    }

    #[test]
    fn a_one_line_output_carries_no_disclosure_glyph() {
        let expanded = Expanded::new();
        let call = output("o1", "Enabled plan mode", &ctx(40, &expanded), &styles());
        assert_eq!(call.lines.len(), 1);
        assert_eq!(call.head, 1);
        assert_eq!(call.lines[0].text(), "  └ Enabled plan mode");
    }

    #[test]
    fn a_multi_line_output_collapses_to_its_first_line_with_a_disclosure_glyph() {
        let expanded = Expanded::new();
        let call = output("o1", "Current Plan\n/home/x.md\n\n# Title", &ctx(40, &expanded), &styles());
        assert_eq!(call.lines.len(), 1);
        assert_eq!(call.lines[0].text(), "▸ Current Plan");
    }

    #[test]
    fn expanding_a_multi_line_output_reveals_the_rest_folded() {
        let mut expanded = Expanded::new();
        expanded.insert(Box::from("o1"));
        let call = output("o1", "one\ntwo\nthree", &ctx(40, &expanded), &styles());
        let texts: Vec<String> = call.lines.iter().map(RenderedLine::text).collect();
        assert_eq!(texts[0], "▾ one");
        assert!(texts.iter().any(|line| line.contains("two")));
        assert!(texts.iter().any(|line| line.contains("three")));
    }

    #[test]
    fn a_long_output_folds_past_twenty_lines_when_expanded() {
        let mut expanded = Expanded::new();
        expanded.insert(Box::from("o1"));
        let text = (0..30).map(|line| format!("line {line}")).collect::<Vec<_>>().join("\n");
        let call = output("o1", &text, &ctx(40, &expanded), &styles());
        assert!(call.lines.iter().any(|line| line.text().contains("more lines")), "{:?}", call.lines);
    }

    #[test]
    fn no_row_ends_in_whitespace_or_overflows_at_any_width() {
        let expanded = Expanded::new();
        for width in 1..=80_usize {
            let call =
                invocation("t1", "/opsx:explore", Some("a fairly long argument string here"), &ctx(width, &expanded), &styles());
            for line in call.lines {
                let text = line.text();
                assert_eq!(text.trim_end(), text, "width {width} left trailing whitespace: {text:?}");
                assert!(text.width() <= width, "width {width} overflowed to {}", text.width());
            }
        }
    }
}
