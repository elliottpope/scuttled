//! Index trait and implementations
//!
//! The Index is responsible for tracking message metadata, performing searches,
//! and managing mailbox state. It returns file paths for the MailStore to use.
//!
//! ## Architecture
//!
//! - `IndexBackend` trait: Low-level storage interface (in-memory, Tantivy, etc.)
//! - `Indexer`: Public coordinating wrapper that adds EventBus integration and write ordering
//! - Backend implementations are hidden; users interact only with `Indexer`

use async_trait::async_trait;
use crate::error::Result;
use crate::types::*;

pub(crate) mod backend;
mod indexer;
pub mod r#impl;

// Re-export public types
pub use indexer::Indexer;

/// Trait for indexing and searching email contents
///
/// NOTE: This trait is kept for backward compatibility but most code should
/// use the concrete `Indexer` type instead of `dyn Index`.
/// The Indexer provides EventBus integration and write ordering automatically.
#[async_trait]
pub trait Index: Send + Sync {
    /// Create a new mailbox
    async fn create_mailbox(&self, username: &str, name: &str) -> Result<()>;

    /// Delete a mailbox
    async fn delete_mailbox(&self, username: &str, name: &str) -> Result<()>;

    /// Rename a mailbox
    async fn rename_mailbox(&self, username: &str, old_name: &str, new_name: &str) -> Result<()>;

    /// List all mailboxes for a user
    async fn list_mailboxes(&self, username: &str) -> Result<Vec<Mailbox>>;

    /// Get mailbox information
    async fn get_mailbox(&self, username: &str, name: &str) -> Result<Option<Mailbox>>;

    /// Add a message to the index
    async fn add_message(&self, username: &str, mailbox: &str, message: IndexedMessage) -> Result<String>;

    /// Get a message by its ID (returns the file path)
    async fn get_message_path(&self, id: MessageId) -> Result<Option<String>>;

    /// Get a message by mailbox and UID (returns the file path)
    async fn get_message_path_by_uid(&self, username: &str, mailbox: &str, uid: Uid) -> Result<Option<String>>;

    /// List all message paths in a mailbox
    async fn list_message_paths(&self, username: &str, mailbox: &str) -> Result<Vec<String>>;

    /// Get message metadata by ID
    async fn get_message_metadata(&self, id: MessageId) -> Result<Option<IndexedMessage>>;

    /// Update message flags
    async fn update_flags(&self, id: MessageId, flags: Vec<MessageFlag>) -> Result<()>;

    /// Delete a message from the index
    async fn delete_message(&self, id: MessageId) -> Result<()>;

    /// Search for messages matching a query (returns file paths)
    async fn search(&self, username: &str, mailbox: &str, query: &SearchQuery) -> Result<Vec<String>>;

    /// Get the next available UID for a mailbox
    async fn get_next_uid(&self, username: &str, mailbox: &str) -> Result<Uid>;

    /// Shutdown the index gracefully
    async fn shutdown(&self) -> Result<()>;
}

/// Message metadata stored in the index
#[derive(Debug, Clone)]
pub struct IndexedMessage {
    pub id: MessageId,
    pub uid: Uid,
    pub mailbox: String,
    pub flags: Vec<MessageFlag>,
    pub internal_date: chrono::DateTime<chrono::Utc>,
    pub size: usize,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body_preview: String,
}
