//! Mailboxes registry trait and implementations
//!
//! The Mailboxes registry maintains shared mailbox state across concurrent sessions,
//! including UID validity, next UID, and root paths.

use async_trait::async_trait;
use crate::error::Result;
use crate::types::*;

pub mod r#impl;

/// Mailbox information stored in the registry
#[derive(Debug, Clone)]
pub struct MailboxInfo {
    pub id: MailboxId,
    pub uid_validity: u32,
    pub next_uid: Uid,
    pub root_path: String,
    pub message_count: u32,
    pub unseen_count: u32,
    pub recent_count: u32,
}

/// Trait for managing mailbox registries
///
/// This trait allows for flexible implementations (in-memory, Redis, etc.)
/// and ensures mailbox state is shared across concurrent sessions.
#[async_trait]
pub trait Mailboxes: Send + Sync {
    /// Create a new mailbox
    async fn create_mailbox(&self, username: &str, name: &str) -> Result<MailboxInfo>;

    /// Get mailbox information
    async fn get_mailbox(&self, username: &str, name: &str) -> Result<Option<MailboxInfo>>;

    /// Delete a mailbox
    async fn delete_mailbox(&self, username: &str, name: &str) -> Result<()>;

    /// List all mailboxes for a user
    async fn list_mailboxes(&self, username: &str) -> Result<Vec<MailboxInfo>>;

    /// Update the next UID for a mailbox
    async fn update_next_uid(&self, id: &MailboxId, next_uid: Uid) -> Result<()>;

    /// Get the next available UID for a mailbox and atomically increment it
    ///
    /// This operation must be atomic to ensure UIDs are unique across
    /// concurrent sessions and instances.
    async fn get_next_uid(&self, username: &str, mailbox: &str) -> Result<Uid>;

    /// Update the UID validity for a mailbox
    async fn update_uid_validity(&self, id: &MailboxId, uid_validity: u32) -> Result<()>;

    /// Update message counts
    async fn update_counts(
        &self,
        id: &MailboxId,
        message_count: u32,
        unseen_count: u32,
        recent_count: u32,
    ) -> Result<()>;

    /// Shutdown the mailboxes registry gracefully
    async fn shutdown(&self) -> Result<()>;
}
