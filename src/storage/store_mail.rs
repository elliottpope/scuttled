//! StoreMail trait for low-level file operations
//!
//! Implementations should be simple, direct file I/O without channels.
//! The Storage layer coordinates channel-based writes.

use crate::error::Result;
use async_trait::async_trait;

/// Trait for basic file operations on email storage
///
/// Implementations should be simple, direct file I/O.
/// Examples: filesystem operations, S3 API calls, etc.
///
/// The Storage layer handles write coordination via channels.
#[async_trait]
pub trait StoreMail: Clone {
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
}
