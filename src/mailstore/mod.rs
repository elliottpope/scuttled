//! MailStore trait and implementations
//!
//! The MailStore is responsible for storing and retrieving raw email messages
//! on disk. It works with file paths provided by the Index.

use async_trait::async_trait;
use crate::error::Result;
use crate::types::MessageId;
use std::path::PathBuf;

pub mod r#impl;
pub mod watcher;

/// Trait for storing raw email messages
#[async_trait]
pub trait MailStore: Send + Sync {
    /// Store a message at the given path
    async fn store(&self, path: &str, content: &[u8]) -> Result<()>;

    /// Retrieve a message from the given path
    async fn retrieve(&self, path: &str) -> Result<Option<Vec<u8>>>;

    /// Delete a message at the given path
    async fn delete(&self, path: &str) -> Result<()>;

    /// Check if a message exists at the given path
    async fn exists(&self, path: &str) -> Result<bool>;

    /// Shutdown the mailstore gracefully
    async fn shutdown(&self) -> Result<()>;
}

/// Events that can occur in the mail store
#[derive(Debug, Clone)]
pub enum MailStoreEvent {
    /// A new message was added
    MessageCreated {
        username: String,
        mailbox: String,
        path: PathBuf,
        content: Vec<u8>,
    },
    /// A message was modified (e.g., flags changed in Maildir)
    MessageModified {
        message_id: MessageId,
        old_path: PathBuf,
        new_path: PathBuf,
    },
    /// A message was deleted
    MessageDeleted {
        message_id: MessageId,
        path: PathBuf,
    },
}

/// Trait for watching changes to the mail store
#[async_trait]
pub trait MailStoreWatcher: Send + Sync {
    /// Start watching a mailbox directory for changes
    async fn watch_mailbox(&self, username: &str, mailbox: &str) -> Result<()>;

    /// Stop watching a mailbox directory
    async fn unwatch_mailbox(&self, username: &str, mailbox: &str) -> Result<()>;

    /// Shutdown the watcher gracefully
    async fn shutdown(&self) -> Result<()>;
}
