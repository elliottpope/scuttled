//! Storage layer for email messages
//!
//! This module provides a Copy/Clone storage handle that coordinates between:
//! - StoreMail: Low-level file operations (write, move, remove)
//! - MailboxFormat: Filename/metadata parsing and construction
//!
//! # Architecture
//!
//! Storage is generic over two traits to separate concerns:
//! - `StoreMail` handles the actual file I/O (filesystem, S3, etc.)
//! - `MailboxFormat` handles filename conventions (Maildir, mbox, etc.)
//!
//! Both are Clone, making Storage cheap to clone without Arc overhead.
//!
//! # Usage
//!
//! ```ignore
//! use scuttled::storage::{Storage, FilesystemStore};
//! use scuttled::mailstore::format::MaildirFormat;
//!
//! let store = FilesystemStore::new("./data/mail").await?;
//! let format = MaildirFormat;
//! let storage = Storage::new(store, format);
//!
//! // Cheap to clone!
//! let storage_clone = storage.clone();
//!
//! // Store a message
//! storage.store("alice/INBOX/msg1.eml", b"From: ...").await?;
//!
//! // Update flags (uses MailboxFormat for filename)
//! storage.update_flags("alice/INBOX/msg1.eml", &[MessageFlag::Seen]).await?;
//! ```

pub mod store_mail;
pub mod filesystem_store;

use crate::error::Result;
use crate::mailstore::format::MailboxFormat;
use crate::types::MessageFlag;
use std::sync::Arc;

pub use store_mail::StoreMail;
pub use filesystem_store::FilesystemStore;

/// Storage coordinator (cheap to clone)
///
/// Generic over:
/// - S: StoreMail implementation (file operations)
/// - F: MailboxFormat implementation (filename conventions)
#[derive(Clone)]
pub struct Storage<S, F>
where
    S: StoreMail,
    F: MailboxFormat,
{
    /// Low-level file storage (Clone via channel sender)
    store: S,
    /// Mailbox format handler (Clone via Arc for trait object)
    format: Arc<F>,
}

impl<S, F> Storage<S, F>
where
    S: StoreMail,
    F: MailboxFormat + 'static,
{
    /// Create a new Storage instance
    pub fn new(store: S, format: F) -> Self {
        Self {
            store,
            format: Arc::new(format),
        }
    }

    /// Store a message at the given path
    pub async fn store(&self, path: &str, content: &[u8]) -> Result<()> {
        self.store.write(path, content).await
    }

    /// Retrieve a message from the given path
    pub async fn retrieve(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.store.read(path).await
    }

    /// Delete a message at the given path
    pub async fn delete(&self, path: &str) -> Result<()> {
        self.store.remove(path).await
    }

    /// Check if a message exists at the given path
    pub async fn exists(&self, path: &str) -> Result<bool> {
        self.store.exists(path).await
    }

    /// Update flags for a message
    ///
    /// This uses the MailboxFormat to construct the new filename with flags,
    /// then uses StoreMail to atomically move the file.
    pub async fn update_flags(&self, current_path: &str, flags: &[MessageFlag]) -> Result<()> {
        // Use the format to determine the new filename
        let flag_component = self.format.flags_to_filename_component(flags);

        // Parse current path to get components
        let path_obj = std::path::Path::new(current_path);
        let filename = path_obj
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| crate::error::Error::Internal(format!("Invalid path: {}", current_path)))?;

        // Extract unique ID (before :2, if present)
        let unique_id = if let Some(idx) = filename.find(":2,") {
            &filename[..idx]
        } else {
            filename
        };

        // Build new filename with flags
        let new_filename = if flag_component.is_empty() {
            unique_id.to_string()
        } else {
            format!("{}{}", unique_id, flag_component)
        };

        // Determine directory (new/ vs cur/ for Maildir)
        let parent = path_obj.parent().unwrap_or(std::path::Path::new(""));
        let parent_name = parent
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        // For Maildir: messages with Seen flag go to cur/, others to new/
        let new_parent = if flags.iter().any(|f| matches!(f, MessageFlag::Seen)) {
            if parent_name == "new" {
                parent.parent().unwrap_or(parent).join("cur")
            } else {
                parent.to_path_buf()
            }
        } else {
            if parent_name == "cur" {
                parent.parent().unwrap_or(parent).join("new")
            } else {
                parent.to_path_buf()
            }
        };

        let new_path = new_parent.join(new_filename);
        let new_path_str = new_path.to_str()
            .ok_or_else(|| crate::error::Error::Internal("Invalid path encoding".to_string()))?;

        // Atomically move the file
        self.store.move_file(current_path, new_path_str).await
    }

    /// Shutdown the storage gracefully
    pub async fn shutdown(&self) -> Result<()> {
        self.store.shutdown().await
    }

    /// Get a reference to the underlying store
    pub fn get_store(&self) -> &S {
        &self.store
    }

    /// Get a reference to the format handler
    pub fn get_format(&self) -> &F {
        &self.format
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mailstore::format::MailboxFormat;
    use tempfile::TempDir;

    // Simple test format that just appends flags
    #[derive(Clone)]
    struct TestFormat;

    impl MailboxFormat for TestFormat {
        fn parse_message_path(&self, _path: &std::path::Path, _root: &std::path::Path) -> Option<crate::mailstore::format::ParsedMessage> {
            None
        }

        fn watch_subdirectories(&self) -> Vec<&'static str> {
            vec!["new", "cur"]
        }

        fn flags_to_filename_component(&self, flags: &[MessageFlag]) -> String {
            if flags.is_empty() {
                String::new()
            } else {
                let mut chars = Vec::new();
                for flag in flags {
                    match flag {
                        MessageFlag::Seen => chars.push('S'),
                        MessageFlag::Flagged => chars.push('F'),
                        _ => {}
                    }
                }
                format!(":2,{}", chars.into_iter().collect::<String>())
            }
        }

        fn is_valid_message_file(&self, _path: &std::path::Path) -> bool {
            true
        }
    }

    #[async_std::test]
    async fn test_storage_creation() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemStore::new(tmp_dir.path()).await.unwrap();
        let storage = Storage::new(store, TestFormat);
        storage.shutdown().await.unwrap();
    }

    #[async_std::test]
    async fn test_storage_store_retrieve() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemStore::new(tmp_dir.path()).await.unwrap();
        let storage = Storage::new(store, TestFormat);

        let content = b"Hello, World!";
        storage.store("test/msg.eml", content).await.unwrap();

        let retrieved = storage.retrieve("test/msg.eml").await.unwrap();
        assert_eq!(retrieved, Some(content.to_vec()));

        storage.shutdown().await.unwrap();
    }

    #[async_std::test]
    async fn test_storage_delete() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemStore::new(tmp_dir.path()).await.unwrap();
        let storage = Storage::new(store, TestFormat);

        storage.store("test/msg.eml", b"content").await.unwrap();
        assert!(storage.exists("test/msg.eml").await.unwrap());

        storage.delete("test/msg.eml").await.unwrap();
        assert!(!storage.exists("test/msg.eml").await.unwrap());

        storage.shutdown().await.unwrap();
    }

    #[async_std::test]
    async fn test_storage_update_flags() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemStore::new(tmp_dir.path()).await.unwrap();
        let storage = Storage::new(store, TestFormat);

        // Store message in new/
        storage
            .store("alice/INBOX/new/msg123", b"content")
            .await
            .unwrap();

        // Mark as seen - should move to cur/ with :2,S
        storage
            .update_flags("alice/INBOX/new/msg123", &[MessageFlag::Seen])
            .await
            .unwrap();

        // Original should not exist
        assert!(!storage.exists("alice/INBOX/new/msg123").await.unwrap());

        // New location should exist
        assert!(storage
            .exists("alice/INBOX/cur/msg123:2,S")
            .await
            .unwrap());

        storage.shutdown().await.unwrap();
    }

    #[async_std::test]
    async fn test_storage_clone() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemStore::new(tmp_dir.path()).await.unwrap();
        let storage1 = Storage::new(store, TestFormat);
        let storage2 = storage1.clone(); // Cheap clone!

        storage1.store("test1.eml", b"from storage1").await.unwrap();
        storage2.store("test2.eml", b"from storage2").await.unwrap();

        // Both should be able to read each other's writes
        assert!(storage1.exists("test2.eml").await.unwrap());
        assert!(storage2.exists("test1.eml").await.unwrap());

        storage1.shutdown().await.unwrap();
    }
}
