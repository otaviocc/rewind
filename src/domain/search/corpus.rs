//! The in-memory search corpus: one shard's raw bytes per indexed project, plus history.
//!
//! Loaded once and cheap to reparse (`Shard::parse`) on every query — see `domain::search::
//! engine`. Loading touches the filesystem and belongs on a worker thread, never the UI
//! thread.

use std::fs;
use std::path::Path;

use crate::domain::cache::meta;
use crate::domain::cache::shard::CACHE_VERSION;

const HISTORY_SHARD_NAME: &str = "_history";
const SHARD_EXTENSION: &str = "shard";
const META_FILE_NAME: &str = "meta.json";
pub const HISTORY_WEIGHT: u32 = 110;
pub const NORMAL_WEIGHT: u32 = 100;

pub struct ShardEntry {
    pub directory: Option<String>,
    pub weight: u32,
    pub bytes: Vec<u8>,
}

#[derive(Default)]
pub struct Corpus {
    pub shards: Vec<ShardEntry>,
}

pub fn load(cache_root: &Path) -> Corpus {
    let version_dir = cache_root.join(format!("v{CACHE_VERSION}"));
    let Some(meta) = meta::read(&version_dir.join(META_FILE_NAME)) else { return Corpus::default() };

    let mut shards = Vec::with_capacity(meta.projects.len().saturating_add(1));
    for project in &meta.projects {
        let path = version_dir.join(&project.directory).with_extension(SHARD_EXTENSION);
        if let Ok(bytes) = fs::read(&path) {
            shards.push(ShardEntry { directory: Some(project.directory.clone()), weight: NORMAL_WEIGHT, bytes });
        }
    }

    let history_path = version_dir.join(HISTORY_SHARD_NAME).with_extension(SHARD_EXTENSION);
    if let Ok(bytes) = fs::read(&history_path) {
        shards.push(ShardEntry { directory: None, weight: HISTORY_WEIGHT, bytes });
    }

    Corpus { shards }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;
    use crate::domain::cache::store;

    #[test]
    fn a_missing_cache_root_loads_an_empty_corpus() {
        let corpus = load(&PathBuf::from("/nonexistent-rewind-search-test"));
        assert!(corpus.shards.is_empty());
    }

    #[test]
    fn a_built_store_loads_one_shard_per_project_plus_history_at_the_higher_weight() {
        let claude = TempDir::new().expect("a temporary directory");
        let project_dir = claude.path().join("projects").join("-Users-fixture-holodeck");
        fs::create_dir_all(&project_dir).expect("a project directory");
        fs::write(
            project_dir.join("s1.jsonl"),
            b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"read the grid\"},\"origin\":{\"kind\":\"human\"}}\n",
        )
        .expect("a written fixture session");
        fs::write(claude.path().join("history.jsonl"), b"{\"display\":\"read the grid\",\"sessionId\":\"s1\"}\n")
            .expect("a written history file");

        let cache = TempDir::new().expect("a temporary directory");
        store::rebuild(claude.path(), cache.path());

        let corpus = load(cache.path());
        assert_eq!(corpus.shards.len(), 2);
        let history = corpus.shards.iter().find(|shard| shard.directory.is_none()).expect("a history shard");
        assert_eq!(history.weight, HISTORY_WEIGHT);
        let project = corpus.shards.iter().find(|shard| shard.directory.is_some()).expect("a project shard");
        assert_eq!(project.weight, NORMAL_WEIGHT);
    }
}
