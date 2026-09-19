//! Ranking: `field_weight × match_quality × (0.6 + 0.4 × recency)`.
//!
//! Fanned out over the corpus with `thread::scope`, one bounded top-K min-heap per shard,
//! merged here. Scores are fixed-point `u64` — see `matcher` for why this crate avoids
//! floats.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::domain::cache::shard::{Field, Kind, Record, Shard};
use crate::domain::search::corpus::{Corpus, ShardEntry};
use crate::domain::search::matcher::Needles;
use crate::domain::search::query::Query;

pub const TOP_K_PER_SHARD: usize = 200;
const RECENCY_WINDOW_MS: i64 = 180 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub score: u64,
    pub directory: Option<String>,
    pub file_name: String,
    pub line_no: u32,
    pub byte_off: u64,
    pub ts_ms: i64,
    pub kind: Kind,
    pub field: Field,
}

pub fn search(corpus: &Corpus, query: &Query, now_ms: i64) -> Vec<Hit> {
    if query.positive.is_empty() {
        return Vec::new();
    }
    let needles = Needles::build(&query.positive, &query.negative);
    if needles.is_empty() {
        return Vec::new();
    }

    let candidates: Vec<&ShardEntry> =
        corpus.shards.iter().filter(|shard| project_allowed(query, shard.directory.as_deref())).collect();

    let mut per_shard: Vec<Vec<Hit>> = Vec::with_capacity(candidates.len());
    std::thread::scope(|scope| {
        let handles: Vec<_> =
            candidates.iter().map(|entry| scope.spawn(|| search_shard(entry, query, &needles, now_ms))).collect();
        for handle in handles {
            if let Ok(hits) = handle.join() {
                per_shard.push(hits);
            }
        }
    });

    let mut merged: Vec<Hit> = per_shard.into_iter().flatten().collect();
    merged.sort_by_key(|hit| Reverse(hit.score));
    merged.truncate(TOP_K_PER_SHARD);
    merged
}

fn project_allowed(query: &Query, directory: Option<&str>) -> bool {
    let Some(wanted) = &query.project else { return true };
    directory.is_some_and(|directory| directory.to_lowercase().contains(wanted.as_str()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ScoredIndex {
    score: u64,
    index: usize,
}

fn search_shard(entry: &ShardEntry, query: &Query, needles: &Needles, now_ms: i64) -> Vec<Hit> {
    let Ok(shard) = Shard::parse(&entry.bytes) else { return Vec::new() };
    let mut heap: BinaryHeap<Reverse<ScoredIndex>> = BinaryHeap::new();

    for (index, record) in shard.records().iter().enumerate() {
        if let Some(category) = query.category
            && !category.matches(record.field)
        {
            continue;
        }
        let Some(text) = shard.text(record) else { continue };
        let Some(quality) = needles.quality(text) else { continue };

        let recency = recency_pct(record.ts_ms, now_ms);
        let value = score(field_weight(record.field), entry.weight, quality, recency);
        push_bounded(&mut heap, ScoredIndex { score: value, index });
    }

    let mut scored: Vec<ScoredIndex> = heap.into_iter().map(|Reverse(item)| item).collect();
    scored.sort_by(|left, right| right.cmp(left));
    scored
        .into_iter()
        .filter_map(|item| shard.records().get(item.index).and_then(|record| to_hit(entry, &shard, record, item.score)))
        .collect()
}

fn push_bounded(heap: &mut BinaryHeap<Reverse<ScoredIndex>>, item: ScoredIndex) {
    if heap.len() < TOP_K_PER_SHARD {
        heap.push(Reverse(item));
        return;
    }
    if let Some(Reverse(min)) = heap.peek()
        && item.score > min.score
    {
        heap.pop();
        heap.push(Reverse(item));
    }
}

fn to_hit(entry: &ShardEntry, shard: &Shard<'_>, record: &Record, score: u64) -> Option<Hit> {
    let file_idx = usize::try_from(record.file_idx).ok()?;
    let file_name = shard.files().get(file_idx)?.path.clone();
    Some(Hit {
        score,
        directory: entry.directory.clone(),
        file_name,
        line_no: record.line_no,
        byte_off: record.byte_off,
        ts_ms: record.ts_ms,
        kind: record.kind,
        field: record.field,
    })
}

const fn field_weight(field: Field) -> u32 {
    match field {
        Field::UserPrompt => 100,
        Field::AssistantText => 60,
        Field::Thinking => 35,
        Field::ToolInput => 30,
        Field::ToolResult => 15,
    }
}

fn recency_pct(ts_ms: i64, now_ms: i64) -> u32 {
    if ts_ms <= 0 {
        return 0;
    }
    if now_ms <= ts_ms {
        return 100;
    }
    let age = now_ms.saturating_sub(ts_ms);
    if age >= RECENCY_WINDOW_MS {
        return 0;
    }
    let remaining = RECENCY_WINDOW_MS.saturating_sub(age);
    u32::try_from(remaining.saturating_mul(100).checked_div(RECENCY_WINDOW_MS).unwrap_or(0)).unwrap_or(0)
}

fn score(field_weight: u32, shard_weight: u32, quality: u32, recency: u32) -> u64 {
    let recency_factor = 60u64.saturating_add(40u64.saturating_mul(u64::from(recency)).checked_div(100).unwrap_or(0));
    u64::from(field_weight)
        .saturating_mul(u64::from(shard_weight))
        .saturating_mul(u64::from(quality))
        .saturating_mul(recency_factor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::cache::shard::Builder;
    use crate::domain::search::query;

    fn corpus_with(records: &[(Kind, Field, &str, i64)]) -> Corpus {
        let mut builder = Builder::new();
        let file = builder.push_file("s1.jsonl", 1, 0, 0, 0);
        for (index, (kind, field, text, ts_ms)) in records.iter().enumerate() {
            let seq = u32::try_from(index).unwrap_or(0);
            builder.push_record(file, seq, u64::from(seq), *ts_ms, *kind, *field, 0, seq, text);
        }
        let bytes = builder.finish(0);
        Corpus { shards: vec![ShardEntry { directory: Some("-a-project".to_owned()), weight: 100, bytes }] }
    }

    #[test]
    fn an_empty_query_returns_nothing() {
        let corpus = corpus_with(&[(Kind::Transcript, Field::UserPrompt, "read the grid scanner", 1000)]);
        let hits = search(&corpus, &query::parse(""), 1000);
        assert!(hits.is_empty());
    }

    #[test]
    fn a_human_prompt_outranks_a_tool_result_for_the_same_term() {
        let corpus = corpus_with(&[
            (Kind::Transcript, Field::ToolResult, "212 src/engine/grid.rs", 1000),
            (Kind::Transcript, Field::UserPrompt, "read the grid scanner back", 1000),
        ]);
        let hits = search(&corpus, &query::parse("grid"), 1000);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits.first().map(|hit| hit.field), Some(Field::UserPrompt));
        assert!(hits.first().map(|hit| hit.score) > hits.get(1).map(|hit| hit.score));
    }

    #[test]
    fn a_negated_term_excludes_a_record_that_would_otherwise_match() {
        let corpus = corpus_with(&[
            (Kind::Transcript, Field::UserPrompt, "the grid is offline", 1000),
            (Kind::Transcript, Field::UserPrompt, "the grid is online", 1000),
        ]);
        let hits = search(&corpus, &query::parse("grid -offline"), 1000);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn an_is_filter_only_keeps_matching_fields() {
        let corpus = corpus_with(&[
            (Kind::Transcript, Field::ToolResult, "grid scanner output", 1000),
            (Kind::Transcript, Field::UserPrompt, "grid scanner reads", 1000),
        ]);
        let hits = search(&corpus, &query::parse("is:tool grid"), 1000);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits.first().map(|hit| hit.field), Some(Field::ToolResult));
    }

    #[test]
    fn a_project_filter_only_keeps_matching_shards() {
        let corpus = corpus_with(&[(Kind::Transcript, Field::UserPrompt, "grid scanner", 1000)]);
        assert!(search(&corpus, &query::parse("project:vade grid"), 1000).is_empty());
        assert_eq!(search(&corpus, &query::parse("project:project grid"), 1000).len(), 1);
    }

    #[test]
    fn recency_breaks_a_tie_between_two_equally_good_matches() {
        let old = (Kind::Transcript, Field::UserPrompt, "the grid scanner reads", 0_i64);
        let recent = (Kind::Transcript, Field::UserPrompt, "the grid scanner reads", 1000_i64);
        let corpus = corpus_with(&[old, recent]);
        let hits = search(&corpus, &query::parse("grid"), 1000);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits.first().map(|hit| hit.byte_off), Some(1), "the more recent duplicate should rank first");
    }

    #[test]
    fn a_far_better_match_quality_still_outranks_a_merely_more_recent_worse_one() {
        let strong_but_stale = (Kind::Transcript, Field::UserPrompt, "the grid scanner reads back", 0_i64);
        let weak_but_fresh = (Kind::Transcript, Field::ToolResult, "gridlock", 1000_i64);
        let corpus = corpus_with(&[strong_but_stale, weak_but_fresh]);
        let hits = search(&corpus, &query::parse("grid"), 1000);
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits.first().map(|hit| hit.byte_off),
            Some(0),
            "field weight and whole-word quality dominate a shallow recency edge"
        );
    }

    #[test]
    fn the_history_shard_is_weighted_above_a_project_shard_for_an_otherwise_equal_hit() {
        let mut builder = Builder::new();
        let file = builder.push_file("history.jsonl", 1, 0, 0, 0);
        builder.push_record(file, 0, 0, 1000, Kind::History, Field::UserPrompt, 0, 0, "grid scanner");
        let history_bytes = builder.finish(0);

        let mut project_corpus = corpus_with(&[(Kind::Transcript, Field::UserPrompt, "grid scanner", 1000)]);
        project_corpus.shards.push(ShardEntry { directory: None, weight: 110, bytes: history_bytes });

        let hits = search(&project_corpus, &query::parse("grid scanner"), 1000);
        assert_eq!(hits.len(), 2);
        assert!(hits.first().is_some_and(|hit| hit.directory.is_none()), "the history hit should rank first");
    }

    #[test]
    fn a_realistic_sized_corpus_still_answers_well_inside_a_keystroke_budget() {
        let words =
            ["grid", "scanner", "reads", "back", "offline", "holodeck", "warp", "core", "interlock", "tricorder", "rewind"];
        let mut shards = Vec::new();
        for shard_index in 0..40 {
            let mut builder = Builder::new();
            let file = builder.push_file("s1.jsonl", 1, 0, 0, 0);
            for record_index in 0_u32..1500 {
                let filler = words.get(usize::try_from(record_index).unwrap_or(0) % words.len()).copied().unwrap_or("grid");
                let text = if record_index % 37 == 0 {
                    format!("grid scanner {filler} record {shard_index} {record_index}")
                } else {
                    format!("{filler} record {shard_index} {record_index}")
                };
                let field = if record_index % 5 == 0 { Field::UserPrompt } else { Field::ToolResult };
                builder.push_record(
                    file,
                    record_index,
                    u64::from(record_index),
                    1000,
                    Kind::Transcript,
                    field,
                    0,
                    record_index,
                    &text,
                );
            }
            shards.push(ShardEntry { directory: Some(format!("-project-{shard_index}")), weight: 100, bytes: builder.finish(0) });
        }
        let corpus = Corpus { shards };

        let started = std::time::Instant::now();
        let hits = search(&corpus, &query::parse("grid scanner"), 1000);
        let elapsed = started.elapsed();

        assert!(!hits.is_empty());
        assert!(elapsed.as_millis() < 200, "a 60k-record corpus took {elapsed:?}, expected well under a keystroke");
    }
}
