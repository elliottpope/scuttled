//! StoreMail trait for low-level file operations
//!
//! This trait defines the basic file operations needed for email storage:
//! - write: Create or overwrite a file
//! - move_file: Rename/move a file atomically
//! - remove: Delete a file
//! - write_metadata: Update file metadata (for flag storage)

use async_trait::async_trait;
use crate::error::Result;

/// Trait for basic file operations on email storage
///
/// Implementations should handle:
/// - Atomic operations
/// - Directory creation
/// - Error handling
#[async_trait]
pub trait StoreMail: Send + Sync + Clone {
    /// Write content to a file at the given path
    ///
    /// Creates parent directories if needed.
    /// If the file exists, it should be overwritten.
    async fn write(&self, path: &str, content: &[u8]) -> Result<()>;

    /// Move a file from one path to another atomically
    ///
    /// This is used for flag updates in Maildir format.
    /// Should be atomic to prevent data loss.
    async fn move_file(&self, from: &str, to: &str) -> Result<()>;

    /// Remove a file at the given path
    ///
    /// Returns an error if the file doesn't exist.
    async fn remove(&self, path: &str) -> Result<()>;

    /// Write metadata to a file
    ///
    /// This can be used for storing flags in filesystem metadata
    /// (e.g., extended attributes) instead of in the filename.
    async fn write_metadata(&self, path: &str, metadata: &[u8]) -> Result<()>;

    /// Check if a file exists at the given path
    async fn exists(&self, path: &str) -> Result<bool>;

    /// Read the contents of a file
    ///
    /// Returns None if the file doesn't exist.
    async fn read(&self, path: &str) -> Result<Option<Vec<u8>>>;

    /// Shutdown the storage backend gracefully
    async fn shutdown(&self) -> Result<()>;
}
