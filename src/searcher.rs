//! Read-only search interface for querying indexed messages
//!
//! The Searcher provides a Clone-able, read-only view of the Index for performing
//! searches without allowing modifications. This is useful for components that only
//! need to query the index but shouldn't be able to modify it.
//!
//! # Architecture
//!
//! Searcher wraps an Arc to the Indexer but only exposes search methods:
//!
//! ```ignore
//! use scuttled::searcher::Searcher;
//!
//! // Create from an Indexer
//! let searcher = Searcher::from_indexer(indexer);
//!
//! // Cheap to clone
//! let searcher_clone = searcher.clone();
//!
//! // Search for messages
//! let results = searcher.search(&query).await?;
//! ```
//!
//! # Usage
//!
//! ```ignore
//! // In a session or command handler
//! let searcher = context.searcher();
//!
//! // Search without worrying about modifying the index
//! let messages = searcher.search(&SearchQuery {
//!     username: "alice",
//!     mailbox: "INBOX",
//!     query: "subject:urgent",
//! }).await?;
//! ```

use std::sync::Arc;

use crate::error::Result;
use crate::index::Indexer;
use crate::types::SearchQuery;

/// Read-only search interface (cheap to clone)
///
/// This struct provides a Clone-able, read-only view of the Index.
/// It wraps an Arc<Indexer> internally, so cloning is cheap.
#[derive(Clone)]
pub struct Searcher {
    indexer: Arc<Indexer>,
}

impl Searcher {
    /// Create a new Searcher from an Indexer
    pub fn from_indexer(indexer: Arc<Indexer>) -> Self {
        Self { indexer }
    }

    /// Search for messages matching the query
    ///
    /// Returns a list of paths to messages that match the search criteria.
    pub async fn search(
        &self,
        username: &str,
        mailbox: &str,
        query: &SearchQuery,
    ) -> Result<Vec<String>> {
        self.indexer.search(username, mailbox, query).await
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::backend::IndexBackend;
    use crate::index::r#impl::inmemory::InMemoryBackend;
    use crate::index::Indexer;

    #[test]
    fn test_searcher_creation() {
        let backend = Box::new(InMemoryBackend::new()) as Box<dyn IndexBackend>;
        let indexer = Indexer::new(backend, None);
        let searcher = Searcher::from_indexer(Arc::new(indexer));

        // Searcher should be clone-able
        let _searcher_clone = searcher.clone();
    }
}
