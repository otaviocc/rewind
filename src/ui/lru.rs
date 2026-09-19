//! A bounded cache of parsed sessions, owned by the loader thread. Keyed on the file's
//! identity at parse time, so a session that has grown since is correctly a miss.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::domain::thread::Conversation;

pub const BUDGET_BYTES: u64 = 256 * 1024 * 1024;
const PARSE_FACTOR: u64 = 3;
const MIN_RETAINED: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    len: u64,
    mtime_ms: i64,
}

struct Entry {
    key: Key,
    charge: u64,
    conversation: Arc<Conversation>,
}

#[derive(Default)]
pub struct SessionLru {
    entries: Vec<Entry>,
    budget: u64,
}

impl SessionLru {
    pub const fn new(budget: u64) -> Self {
        Self { entries: Vec::new(), budget }
    }

    pub fn get(&mut self, path: &Path, len: u64, mtime_ms: i64) -> Option<Arc<Conversation>> {
        let key = Key { path: path.to_path_buf(), len, mtime_ms };
        let index = self.entries.iter().position(|entry| entry.key == key)?;
        let entry = self.entries.remove(index);
        let conversation = Arc::clone(&entry.conversation);
        self.entries.push(entry);
        Some(conversation)
    }

    pub fn insert(&mut self, path: &Path, len: u64, mtime_ms: i64, conversation: Arc<Conversation>) {
        let key = Key { path: path.to_path_buf(), len, mtime_ms };
        self.entries.retain(|entry| entry.key != key);
        let charge = len.saturating_mul(PARSE_FACTOR);
        self.entries.push(Entry { key, charge, conversation });
        self.evict();
    }

    fn evict(&mut self) {
        while self.entries.len() > MIN_RETAINED && self.total_charge() > self.budget {
            if self.entries.is_empty() {
                break;
            }
            self.entries.remove(0);
        }
    }

    fn total_charge(&self) -> u64 {
        self.entries.iter().map(|entry| entry.charge).fold(0_u64, u64::saturating_add)
    }

    #[cfg(test)]
    const fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    fn contains(&self, path: &Path, len: u64, mtime_ms: i64) -> bool {
        let key = Key { path: path.to_path_buf(), len, mtime_ms };
        self.entries.iter().any(|entry| entry.key == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::TempDir;

    fn conversation() -> Arc<Conversation> {
        let dir = TempDir::new().expect("a temp dir");
        let path = dir.path().join("session.jsonl");
        fs::write(
            &path,
            r#"{"parentUuid":null,"isSidechain":false,"message":{"role":"user","content":"hi"},"type":"user","origin":{"kind":"human"},"uuid":"u1","timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#,
        )
        .expect("a written fixture session");
        Arc::new(crate::domain::thread::build(&path).expect("a conversation"))
    }

    #[test]
    fn a_fresh_entry_is_retrievable() {
        let mut lru = SessionLru::new(BUDGET_BYTES);
        lru.insert(Path::new("/a"), 100, 1, conversation());
        assert!(lru.get(Path::new("/a"), 100, 1).is_some());
    }

    #[test]
    fn a_grown_session_is_a_miss_at_its_old_length() {
        let mut lru = SessionLru::new(BUDGET_BYTES);
        lru.insert(Path::new("/a"), 100, 1, conversation());
        assert!(lru.get(Path::new("/a"), 200, 1).is_none());
    }

    #[test]
    fn a_changed_mtime_is_a_miss() {
        let mut lru = SessionLru::new(BUDGET_BYTES);
        lru.insert(Path::new("/a"), 100, 1, conversation());
        assert!(lru.get(Path::new("/a"), 100, 2).is_none());
    }

    #[test]
    fn eviction_drops_the_oldest_entry_first_once_over_budget() {
        let mut lru = SessionLru::new(250);
        lru.insert(Path::new("/a"), 100, 1, conversation());
        lru.insert(Path::new("/b"), 100, 1, conversation());
        lru.insert(Path::new("/c"), 100, 1, conversation());

        assert!(!lru.contains(Path::new("/a"), 100, 1), "the oldest entry must be evicted first");
        assert!(lru.contains(Path::new("/b"), 100, 1));
        assert!(lru.contains(Path::new("/c"), 100, 1));
    }

    #[test]
    fn the_two_most_recent_entries_always_survive_even_over_budget() {
        let mut lru = SessionLru::new(1);
        lru.insert(Path::new("/a"), 1_000_000, 1, conversation());
        lru.insert(Path::new("/b"), 1_000_000, 1, conversation());

        assert_eq!(lru.len(), 2, "the two most recent entries must always be retained");
    }

    #[test]
    fn getting_an_entry_marks_it_as_recently_used() {
        let mut lru = SessionLru::new(250);
        lru.insert(Path::new("/a"), 100, 1, conversation());
        lru.insert(Path::new("/b"), 100, 1, conversation());
        assert!(lru.get(Path::new("/a"), 100, 1).is_some(), "warm the first entry");
        lru.insert(Path::new("/c"), 100, 1, conversation());

        assert!(lru.contains(Path::new("/a"), 100, 1), "recently used entries are not evicted first");
        assert!(!lru.contains(Path::new("/b"), 100, 1), "the least recently used entry is evicted instead");
    }

    #[test]
    fn reinserting_the_same_key_replaces_rather_than_duplicates() {
        let mut lru = SessionLru::new(BUDGET_BYTES);
        lru.insert(Path::new("/a"), 100, 1, conversation());
        lru.insert(Path::new("/a"), 100, 1, conversation());
        assert_eq!(lru.len(), 1);
    }
}
