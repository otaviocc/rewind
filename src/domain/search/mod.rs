//! Full-text search over the in-memory corpus: query parsing, matching, ranking, and
//! resolving a hit back to something the UI can open.

pub mod corpus;
pub mod engine;
pub mod fuzzy;
pub mod matcher;
pub mod query;
pub mod resolve;
