//! The in-memory search corpus: one shard's raw bytes per indexed project, plus history.
//!
//! Loaded once and cheap to reparse (`Shard::parse`) on every query — see `domain::search::
//! engine`. Loading touches the filesystem and belongs on a worker thread, never the UI
//! thread.
//!
//! Enumerated straight off `*.shard` files in the version directory, never through
//! `meta.json` — meta is written only once, after the *whole* scan finishes, so a load that
//! depended on it would see nothing for a scan still in progress. A `.shard` file is safe to
//! read the moment it exists: `atomic::write` only ever renames a complete file into place.

use std::fs;
use std::path::Path;

use crate::domain::cache::shard::CACHE_VERSION;

const HISTORY_SHARD_NAME: &str = "_history";
const SHARD_EXTENSION: &str = "shard";
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

impl Corpus {
    pub fn upsert(&mut self, entry: ShardEntry) {
        if let Some(existing) = self.shards.iter_mut().find(|shard| shard.directory == entry.directory) {
            *existing = entry;
        } else {
            self.shards.push(entry);
        }
    }
}

pub fn load(cache_root: &Path) -> Corpus {
    let version_dir = cache_root.join(format!("v{CACHE_VERSION}"));
    let Ok(entries) = fs::read_dir(&version_dir) else { return Corpus::default() };

    let mut shards = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some(SHARD_EXTENSION) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else { continue };
        let Ok(bytes) = fs::read(&path) else { continue };

        if stem == HISTORY_SHARD_NAME {
            shards.push(ShardEntry { directory: None, weight: HISTORY_WEIGHT, bytes });
        } else {
            shards.push(ShardEntry { directory: Some(stem.to_owned()), weight: NORMAL_WEIGHT, bytes });
        }
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

    #[test]
    fn a_shard_on_disk_loads_even_before_meta_json_is_ever_written() {
        let claude = TempDir::new().expect("a temporary directory");
        let project_dir = claude.path().join("projects").join("-Users-fixture-holodeck");
        fs::create_dir_all(&project_dir).expect("a project directory");
        fs::write(
            project_dir.join("s1.jsonl"),
            b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"read the grid\"},\"origin\":{\"kind\":\"human\"}}\n",
        )
        .expect("a written fixture session");

        let cache = TempDir::new().expect("a temporary directory");
        let plan = store::prepare(claude.path(), cache.path()).expect("a preparable cache root");
        let projects = crate::domain::project::discover(claude.path()).expect("discoverable projects");
        let project = projects.first().expect("one project");
        store::build_project(&plan, project, &mut crate::domain::cache::build::Control::inert());

        assert!(!cache.path().join(format!("v{CACHE_VERSION}")).join("meta.json").is_file(), "meta.json is never written here");
        let corpus = load(cache.path());
        assert_eq!(corpus.shards.len(), 1, "the shard on disk must load without meta.json to point at it");
    }

    #[test]
    fn upserting_a_shard_for_a_known_directory_replaces_it_rather_than_duplicating() {
        let mut corpus = Corpus::default();
        corpus.upsert(ShardEntry { directory: Some("-a".to_owned()), weight: NORMAL_WEIGHT, bytes: vec![1] });
        corpus.upsert(ShardEntry { directory: Some("-a".to_owned()), weight: NORMAL_WEIGHT, bytes: vec![2] });

        assert_eq!(corpus.shards.len(), 1);
        assert_eq!(corpus.shards.first().map(|shard| &shard.bytes), Some(&vec![2]));
    }

    #[test]
    fn upserting_a_shard_for_a_new_directory_appends_it() {
        let mut corpus = Corpus::default();
        corpus.upsert(ShardEntry { directory: Some("-a".to_owned()), weight: NORMAL_WEIGHT, bytes: vec![1] });
        corpus.upsert(ShardEntry { directory: Some("-b".to_owned()), weight: NORMAL_WEIGHT, bytes: vec![2] });

        assert_eq!(corpus.shards.len(), 2);
    }

    #[test]
    fn the_history_shard_upserts_by_its_none_directory_key() {
        let mut corpus = Corpus::default();
        corpus.upsert(ShardEntry { directory: None, weight: HISTORY_WEIGHT, bytes: vec![1] });
        corpus.upsert(ShardEntry { directory: None, weight: HISTORY_WEIGHT, bytes: vec![2] });

        assert_eq!(corpus.shards.len(), 1);
        assert_eq!(corpus.shards.first().map(|shard| &shard.bytes), Some(&vec![2]));
    }
}
