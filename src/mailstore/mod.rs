//! MailStore trait and implementations
//!
//! The MailStore is responsible for storing and retrieving raw email messages
//! on disk. It works with file paths provided by the Index.

use async_trait::async_trait;
use crate::error::Result;
use crate::types::*;

pub mod r#impl;

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
