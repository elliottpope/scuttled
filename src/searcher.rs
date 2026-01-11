//! Read-only search interface for querying indexed messages
//!
//! The Searcher provides a Clone-able, read-only interface for searching messages.
//! It is completely separate from the Indexer (write interface) to enforce read/write separation.
//!
//! # Architecture
//!
//! Searcher is generic over a SearchBackend trait that provides read-only query operations:
//!
//! ```ignore
//! use scuttled::searcher::{Searcher, SearchBackend};
//!
//! // Create with a search backend
//! let searcher = Searcher::new(backend);
//!
//! // Cheap to clone
//! let searcher_clone = searcher.clone();
//!
//! // Search for messages
//! let results = searcher.search("alice", "INBOX", &query).await?;
//! ```
//!
//! # Read/Write Separation
//!
//! - **Searcher**: Read-only queries (this module)
//! - **Indexer**: Write operations + event coordination (separate)
//! - **Backend**: Underlying storage that both can reference
//!
//! This separation ensures that components with only read access
//! cannot accidentally modify the index.

use async_trait::async_trait;
use std::sync::Arc;

use crate::error::Result;
use crate::types::SearchQuery;

/// Backend trait for read-only search operations
///
/// Implementations provide query capabilities without mutation.
/// Both in-memory and persistent backends can implement this trait.
#[async_trait]
pub trait SearchBackend: Send + Sync {
    /// Search for messages matching the query
    ///
    /// Returns paths to messages that match the search criteria.
    async fn search(
        &self,
        username: &str,
        mailbox: &str,
        query: &SearchQuery,
    ) -> Result<Vec<String>>;

    /// List all mailboxes for a user
    async fn list_mailboxes(&self, username: &str) -> Result<Vec<String>>;

    /// Get message count for a mailbox
    async fn message_count(&self, username: &str, mailbox: &str) -> Result<usize>;

    /// Check if a mailbox exists
    async fn mailbox_exists(&self, username: &str, mailbox: &str) -> Result<bool>;
}

/// Read-only search interface (cheap to clone)
///
/// This struct provides a Clone-able, read-only view for searching messages.
/// It wraps a SearchBackend in an Arc, making cloning cheap.
#[derive(Clone)]
pub struct Searcher {
    backend: Arc<dyn SearchBackend>,
}

impl Searcher {
    /// Create a new Searcher from a SearchBackend
    pub fn new(backend: Arc<dyn SearchBackend>) -> Self {
        Self { backend }
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
        self.backend.search(username, mailbox, query).await
    }

    /// List all mailboxes for a user
    pub async fn list_mailboxes(&self, username: &str) -> Result<Vec<String>> {
        self.backend.list_mailboxes(username).await
    }

    /// Get message count for a mailbox
    pub async fn message_count(&self, username: &str, mailbox: &str) -> Result<usize> {
        self.backend.message_count(username, mailbox).await
    }

    /// Check if a mailbox exists
    pub async fn mailbox_exists(&self, username: &str, mailbox: &str) -> Result<bool> {
        self.backend.mailbox_exists(username, mailbox).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mock backend for testing
    struct MockSearchBackend;

    #[async_trait]
    impl SearchBackend for MockSearchBackend {
        async fn search(
            &self,
            _username: &str,
            _mailbox: &str,
            _query: &SearchQuery,
        ) -> Result<Vec<String>> {
            Ok(vec![])
        }

        async fn list_mailboxes(&self, _username: &str) -> Result<Vec<String>> {
            Ok(vec!["INBOX".to_string()])
        }

        async fn message_count(&self, _username: &str, _mailbox: &str) -> Result<usize> {
            Ok(0)
        }

        async fn mailbox_exists(&self, _username: &str, _mailbox: &str) -> Result<bool> {
            Ok(true)
        }
    }

    #[tokio::test]
    async fn test_searcher_creation() {
        let backend = Arc::new(MockSearchBackend);
        let searcher = Searcher::new(backend);

        // Searcher should be clone-able
        let _searcher_clone = searcher.clone();
    }

    #[tokio::test]
    async fn test_searcher_search() {
        let backend = Arc::new(MockSearchBackend);
        let searcher = Searcher::new(backend);

        let query = SearchQuery::All;
        let results = searcher.search("alice", "INBOX", &query).await.unwrap();
        assert_eq!(results.len(), 0);
    }

    #[tokio::test]
    async fn test_searcher_list_mailboxes() {
        let backend = Arc::new(MockSearchBackend);
        let searcher = Searcher::new(backend);

        let mailboxes = searcher.list_mailboxes("alice").await.unwrap();
        assert_eq!(mailboxes, vec!["INBOX"]);
    }
}
