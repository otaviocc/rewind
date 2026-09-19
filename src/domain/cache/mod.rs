//! The search corpus: a bounds-checked binary cursor and the shard format built on it.

pub mod atomic;
pub mod build;
pub mod cursor;
pub mod fnv;
pub mod meta;
pub mod shard;
pub mod store;
