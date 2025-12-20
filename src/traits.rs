//! Trait abstractions for the IMAP server components

use async_trait::async_trait;
use uuid::Uuid;
use crate::types::*;
use crate::error::Result;

/// Trait for storing raw email messages
#[async_trait]
pub trait MailStore: Send + Sync {
    /// Store a new message in the specified mailbox
    async fn store_message(
        &self,
        mailbox: &str,
        message: &Message,
    ) -> Result<()>;

    /// Retrieve a message by its ID
    async fn get_message(&self, id: MessageId) -> Result<Option<Message>>;

    /// Retrieve a message by mailbox and UID
    async fn get_message_by_uid(&self, mailbox: &str, uid: Uid) -> Result<Option<Message>>;

    /// List all messages in a mailbox
    async fn list_messages(&self, mailbox: &str) -> Result<Vec<Message>>;

    /// Delete a message
    async fn delete_message(&self, id: MessageId) -> Result<()>;

    /// Update message flags
    async fn update_flags(&self, id: MessageId, flags: Vec<MessageFlag>) -> Result<()>;

    /// Get the next available UID for a mailbox
    async fn get_next_uid(&self, mailbox: &str) -> Result<Uid>;

    /// Create a new mailbox
    async fn create_mailbox(&self, name: &str) -> Result<()>;

    /// Delete a mailbox
    async fn delete_mailbox(&self, name: &str) -> Result<()>;

    /// List all mailboxes for a user
    async fn list_mailboxes(&self, username: &str) -> Result<Vec<Mailbox>>;

    /// Get mailbox information
    async fn get_mailbox(&self, name: &str) -> Result<Option<Mailbox>>;

    /// Rename a mailbox
    async fn rename_mailbox(&self, old_name: &str, new_name: &str) -> Result<()>;
}

/// Trait for indexing and searching email contents
#[async_trait]
pub trait Index: Send + Sync {
    /// Index a message
    async fn index_message(&self, message: &Message) -> Result<()>;

    /// Remove a message from the index
    async fn remove_message(&self, id: MessageId) -> Result<()>;

    /// Search for messages matching a query
    async fn search(&self, mailbox: &str, query: &SearchQuery) -> Result<Vec<MessageId>>;

    /// Update the index for a message (e.g., when flags change)
    async fn update_message(&self, message: &Message) -> Result<()>;

    /// Clear all indexed messages for a mailbox
    async fn clear_mailbox(&self, mailbox: &str) -> Result<()>;
}

/// Trait for authenticating IMAP connections
#[async_trait]
pub trait Authenticator: Send + Sync {
    /// Authenticate a user with credentials
    async fn authenticate(&self, credentials: &Credentials) -> Result<bool>;

    /// Validate an authentication token (for future OAUTH support)
    async fn validate_token(&self, token: &str) -> Result<Option<Username>>;
}

/// Trait for storing user information
#[async_trait]
pub trait UserStore: Send + Sync {
    /// Create a new user
    async fn create_user(&self, username: &str, password: &str) -> Result<()>;

    /// Get user information
    async fn get_user(&self, username: &str) -> Result<Option<User>>;

    /// Update user password
    async fn update_password(&self, username: &str, new_password: &str) -> Result<()>;

    /// Delete a user
    async fn delete_user(&self, username: &str) -> Result<()>;

    /// List all users
    async fn list_users(&self) -> Result<Vec<Username>>;

    /// Verify a password for a user
    async fn verify_password(&self, username: &str, password: &str) -> Result<bool>;
}

/// Trait for queueing asynchronous tasks
#[async_trait]
pub trait Queue: Send + Sync {
    /// Enqueue a task
    async fn enqueue(&self, task: QueueTask) -> Result<()>;

    /// Dequeue the next task
    async fn dequeue(&self) -> Result<Option<QueueTask>>;

    /// Peek at the next task without removing it
    async fn peek(&self) -> Result<Option<QueueTask>>;

    /// Get the number of pending tasks
    async fn len(&self) -> Result<usize>;

    /// Check if the queue is empty
    async fn is_empty(&self) -> Result<bool>;

    /// Remove a specific task by ID
    async fn remove(&self, id: Uuid) -> Result<bool>;

    /// Clear all tasks
    async fn clear(&self) -> Result<()>;
}
