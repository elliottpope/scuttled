//! Index backend implementations

pub mod inmemory;
pub mod tantivy;

// Re-export for backward compatibility
pub use inmemory::{
    create_inmemory_index, create_inmemory_index_with_eventbus, InMemoryBackend, InMemoryIndex,
};
pub use tantivy::TantivyBackedIndex;
