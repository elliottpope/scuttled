//! Low-level index storage backend trait
//!
//! This trait defines the storage interface that index implementations must provide.
//! It focuses on message indexing and search, with minimal mailbox state tracking
//! needed for message operations (like UID assignment).

use async_trait::async_trait;
use crate::error::Result;
use crate::index::IndexedMessage;
use crate::types::*;

/// Low-level storage backend for the index
///
/// Implementations provide the actual storage mechanism (in-memory, Tantivy, Elasticsearch, etc.)
/// and focus purely on storage operations without EventBus or coordination concerns.
///
/// Note: This trait is internal to the index module. External code should use `Indexer`.
#[async_trait]
pub(crate) trait IndexBackend: Send + Sync {
    // Mailbox lifecycle (minimal - just what Index needs for message operations)

    /// Initialize a mailbox for indexing (creates uid counter, etc.)
    async fn initialize_mailbox(&mut self, username: &str, mailbox: &str) -> Result<()>;

    /// Remove all indexed data for a mailbox
    async fn remove_mailbox_data(&mut self, username: &str, mailbox: &str) -> Result<()>;

    /// Check if a mailbox exists in the index
    async fn mailbox_exists(&self, username: &str, mailbox: &str) -> Result<bool>;

    /// Get the next available UID for a mailbox (and increment the counter)
    async fn get_next_uid(&mut self, username: &str, mailbox: &str) -> Result<Uid>;

    // Message indexing operations

    /// Add a message to the index
    /// Returns the path where the message should be stored
    async fn add_message(
        &mut self,
        username: &str,
        mailbox: &str,
        message: IndexedMessage,
    ) -> Result<String>;

    /// Update message flags
    async fn update_flags(&mut self, id: MessageId, flags: Vec<MessageFlag>) -> Result<()>;

    /// Delete a message from the index
    async fn delete_message(&mut self, id: MessageId) -> Result<()>;

    // Message retrieval operations

    /// Get the path to a message by its ID
    async fn get_message_path(&self, id: MessageId) -> Result<Option<String>>;

    /// Get the path to a message by username, mailbox, and UID
    async fn get_message_path_by_uid(
        &self,
        username: &str,
        mailbox: &str,
        uid: Uid,
    ) -> Result<Option<String>>;

    /// List all message paths in a mailbox
    async fn list_message_paths(&self, username: &str, mailbox: &str) -> Result<Vec<String>>;

    /// Get message metadata by ID
    async fn get_message_metadata(&self, id: MessageId) -> Result<Option<IndexedMessage>>;

    // Search operations

    /// Search for messages matching a query
    /// Returns paths to matching messages
    async fn search(
        &self,
        username: &str,
        mailbox: &str,
        query: &SearchQuery,
    ) -> Result<Vec<String>>;
}
