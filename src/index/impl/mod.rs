//! Index implementations

pub mod inmemory;
pub mod tantivy;

pub use inmemory::InMemoryIndex;
pub use tantivy::TantivyBackedIndex;
