//! The seams a session has that a transcript would otherwise hide: where it started, where it
//! was cleared, where it was compacted, and where a record lost its parent.

use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use crate::domain::record::CompactMetadata;
use crate::domain::thread::Divider;
use crate::render::line::{RenderedLine, StyledSpan, truncate};

const RULE: &str = "─";
const GAP: &str = " ";
const SEPARATOR: &str = " · ";
const LEAST_RULE: usize = 2;

pub fn label(divider: Divider, metadata: Option<&CompactMetadata>) -> Option<String> {
    match divider {
        Divider::SessionStart => None,
        Divider::Clear => Some("cleared".to_owned()),
        Divider::Detached => Some("detached · its parent is not in this file".to_owned()),
        Divider::Compacted => Some(compacted(metadata)),
    }
}

fn compacted(metadata: Option<&CompactMetadata>) -> String {
    let Some(metadata) = metadata else { return "compacted".to_owned() };
    let mut parts = vec!["compacted".to_owned()];
    if let (Some(before), Some(after)) = (metadata.pre_tokens, metadata.post_tokens) {
        parts.push(format!("{} → {} tokens", tokens(before), tokens(after)));
    }
    if let Some(dropped) = metadata.cumulative_dropped_tokens.filter(|dropped| *dropped > 0) {
        parts.push(format!("{} dropped", tokens(dropped)));
    }
    if let Some(elapsed) = metadata.duration_ms.filter(|elapsed| *elapsed > 0) {
        parts.push(duration(elapsed));
    }
    parts.join(SEPARATOR)
}

fn tokens(count: u64) -> String {
    if count < 1_000 {
        return count.to_string();
    }
    if count < 1_000_000 {
        return format!("{}k", count.checked_div(1_000).unwrap_or(0));
    }
    let tenths = count.checked_div(100_000).unwrap_or(0);
    let whole = tenths.checked_div(10).unwrap_or(0);
    let fraction = tenths.checked_rem(10).unwrap_or(0);
    if fraction == 0 { format!("{whole}M") } else { format!("{whole}.{fraction}M") }
}

fn duration(millis: u64) -> String {
    let seconds = millis.checked_div(1_000).unwrap_or(0);
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds.checked_div(60).unwrap_or(0);
    let rest = seconds.checked_rem(60).unwrap_or(0);
    format!("{minutes}m{rest:02}s")
}

pub fn line(text: &str, width: usize, style: Style) -> RenderedLine {
    let mut line = RenderedLine::blank();
    let spent = text.width().saturating_add(GAP.width().saturating_mul(2));
    if spent.saturating_add(LEAST_RULE.saturating_mul(2)) > width {
        line.push(StyledSpan::new(truncate(text, width), style));
        return line;
    }
    let rule = width.saturating_sub(spent);
    let head = rule.checked_div(2).unwrap_or(0);
    let tail = rule.saturating_sub(head);
    line.push(StyledSpan::new(RULE.repeat(head), style));
    line.push(StyledSpan::new(format!("{GAP}{text}{GAP}"), style));
    line.push(StyledSpan::new(RULE.repeat(tail), style));
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> CompactMetadata {
        CompactMetadata {
            trigger: Some("auto".to_owned()),
            pre_tokens: Some(948_649),
            post_tokens: Some(124_177),
            cumulative_dropped_tokens: Some(824_472),
            duration_ms: Some(211_043),
            pre_compact_discovered_tools: Vec::new(),
            preserved_messages: None,
        }
    }

    #[test]
    fn a_compaction_says_what_it_cost() {
        assert_eq!(
            label(Divider::Compacted, Some(&metadata())).as_deref(),
            Some("compacted · 948k → 124k tokens · 824k dropped · 3m31s")
        );
    }

    #[test]
    fn a_compaction_with_no_metadata_still_says_it_happened() {
        assert_eq!(label(Divider::Compacted, None).as_deref(), Some("compacted"));
        assert_eq!(label(Divider::Compacted, Some(&CompactMetadata::default())).as_deref(), Some("compacted"));
    }

    #[test]
    fn a_compaction_drops_the_parts_it_has_no_numbers_for() {
        let sparse = CompactMetadata { pre_tokens: Some(2_400), post_tokens: Some(900), ..CompactMetadata::default() };
        assert_eq!(label(Divider::Compacted, Some(&sparse)).as_deref(), Some("compacted · 2k → 900 tokens"));
    }

    #[test]
    fn the_session_start_is_the_one_divider_with_nothing_to_say() {
        assert_eq!(label(Divider::SessionStart, None), None, "the top of a transcript needs no announcing");
        assert_eq!(label(Divider::Clear, None).as_deref(), Some("cleared"));
        assert!(label(Divider::Detached, None).is_some_and(|text| text.starts_with("detached")));
    }

    #[test]
    fn tokens_read_the_way_a_context_meter_reports_them() {
        assert_eq!(tokens(0), "0");
        assert_eq!(tokens(999), "999");
        assert_eq!(tokens(1_000), "1k");
        assert_eq!(tokens(948_649), "948k");
        assert_eq!(tokens(1_000_000), "1M");
        assert_eq!(tokens(1_450_000), "1.4M");
    }

    #[test]
    fn durations_read_as_minutes_and_seconds_past_a_minute() {
        assert_eq!(duration(0), "0s");
        assert_eq!(duration(9_400), "9s");
        assert_eq!(duration(60_000), "1m00s");
        assert_eq!(duration(211_043), "3m31s");
    }

    #[test]
    fn a_divider_centres_its_label_between_two_rules() {
        let text = line("cleared", 21, Style::new()).text();
        assert_eq!(text, "────── cleared ──────");
        assert_eq!(text.width(), 21);
    }

    #[test]
    fn a_divider_too_narrow_for_a_rule_keeps_the_label() {
        let text = line("compacted · 948k → 124k tokens", 12, Style::new()).text();
        assert_eq!(text, "compacted ·…");
        assert!(text.width() <= 12);
    }

    #[test]
    fn no_divider_overflows_or_ends_in_whitespace_at_any_width() {
        for width in 1..=120_usize {
            for divider in [Divider::Clear, Divider::Detached, Divider::Compacted] {
                let Some(text) = label(divider, Some(&metadata())) else { continue };
                let rendered = line(&text, width, Style::new()).text();
                assert!(rendered.width() <= width, "width {width} overflowed to {}", rendered.width());
                assert_eq!(rendered.trim_end(), rendered, "width {width} left trailing whitespace: {rendered:?}");
            }
        }
    }
}
