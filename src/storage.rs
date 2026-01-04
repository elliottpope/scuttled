//! Storage layer for email messages
//!
//! This module provides a Copy/Clone storage handle that coordinates between:
//! - StoreMail: Low-level file operations (simple, direct I/O)
//! - MailboxFormat: Filename/metadata parsing and construction
//!
//! # Architecture
//!
//! Storage coordinates write operations via channels for atomicity:
//! - StoreMail implementations are simple, direct file I/O (no channels)
//! - Storage layer adds channel-based write coordination
//! - MailboxFormat is lightweight and Clone-able
//!
//! # Usage
//!
//! ```ignore
//! use scuttled::storage::{Storage, FilesystemStore};
//! use scuttled::mailstore::format::MaildirFormat;
//!
//! let store = FilesystemStore::new("./data/mail").await?;
//! let format = MaildirFormat;
//! let storage = Storage::new(store, format).await?;
//!
//! // Cheap to clone!
//! let storage_clone = storage.clone();
//!
//! // Store a message (write-once via channel)
//! storage.store("alice/INBOX/msg1.eml", b"From: ...").await?;
//!
//! // Update flags (uses MailboxFormat for filename)
//! storage.update_flags("alice/INBOX/msg1.eml", &[MessageFlag::Seen]).await?;
//! ```

pub mod store_mail;
pub mod filesystem_store;

use async_std::channel::{bounded, Receiver, Sender};
use async_std::task;
use futures::channel::oneshot;

use crate::error::{Error, Result};
use crate::mailstore::format::MailboxFormat;
use crate::types::MessageFlag;

pub use store_mail::StoreMail;
pub use filesystem_store::FilesystemStore;

/// Storage commands for the writer loop
#[derive(Debug)]
enum StorageCommand {
    Write {
        path: String,
        content: Vec<u8>,
        reply: oneshot::Sender<Result<()>>,
    },
    Move {
        from: String,
        to: String,
        reply: oneshot::Sender<Result<()>>,
    },
    Remove {
        path: String,
        reply: oneshot::Sender<Result<()>>,
    },
    WriteMetadata {
        path: String,
        metadata: Vec<u8>,
        reply: oneshot::Sender<Result<()>>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

/// Storage coordinator (cheap to clone)
///
/// Generic over:
/// - S: StoreMail implementation (file operations)
/// - F: MailboxFormat implementation (filename conventions)
///
/// Coordinates channel-based writes for atomicity.
#[derive(Clone)]
pub struct Storage<S, F>
where
    S: StoreMail,
    F: MailboxFormat + Clone,
{
    /// Low-level file storage (Clone, direct I/O)
    store: S,
    /// Mailbox format handler (Clone, lightweight)
    format: F,
    /// Command sender for write coordination (Clone via channel)
    command_tx: Sender<StorageCommand>,
}

impl<S, F> Storage<S, F>
where
    S: StoreMail + 'static,
    F: MailboxFormat + Clone + Send + Sync + 'static,
{
    /// Create a new Storage instance
    ///
    /// This spawns a writer loop task and returns a handle that can be cloned cheaply.
    pub async fn new(store: S, format: F) -> Result<Self> {
        let (command_tx, command_rx) = bounded(100);

        // Spawn writer loop for write coordination
        let store_clone = store.clone();
        task::spawn(storage_writer_loop(command_rx, store_clone));

        Ok(Self {
            store: store.clone(),
            format,
            command_tx,
        })
    }

    /// Store a message at the given path (write-once via channel)
    pub async fn store(&self, path: &str, content: &[u8]) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(StorageCommand::Write {
                path: path.to_string(),
                content: content.to_vec(),
                reply: tx,
            })
            .await
            .map_err(|_| Error::Internal("Storage writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Storage writer loop dropped reply".to_string()))?
    }

    /// Retrieve a message from the given path (direct read, no channel)
    pub async fn retrieve(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.store.read(path).await
    }

    /// Delete a message at the given path (via channel)
    pub async fn delete(&self, path: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(StorageCommand::Remove {
                path: path.to_string(),
                reply: tx,
            })
            .await
            .map_err(|_| Error::Internal("Storage writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Storage writer loop dropped reply".to_string()))?
    }

    /// Check if a message exists at the given path (direct read, no channel)
    pub async fn exists(&self, path: &str) -> Result<bool> {
        self.store.exists(path).await
    }

    /// Update flags for a message (via channel)
    ///
    /// This uses the MailboxFormat to construct the new filename with flags,
    /// then coordinates the atomic move via the writer loop.
    pub async fn update_flags(&self, current_path: &str, flags: &[MessageFlag]) -> Result<()> {
        // Use the format to determine the new filename
        let flag_component = self.format.flags_to_filename_component(flags);

        // Parse current path to get components
        let path_obj = std::path::Path::new(current_path);
        let filename = path_obj
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| Error::Internal(format!("Invalid path: {}", current_path)))?;

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
            .ok_or_else(|| Error::Internal("Invalid path encoding".to_string()))?;

        // Atomically move the file via writer loop
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(StorageCommand::Move {
                from: current_path.to_string(),
                to: new_path_str.to_string(),
                reply: tx,
            })
            .await
            .map_err(|_| Error::Internal("Storage writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Storage writer loop dropped reply".to_string()))?
    }

    /// Shutdown the storage gracefully
    pub async fn shutdown(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let _ = self.command_tx.send(StorageCommand::Shutdown { reply: tx }).await;
        let _ = rx.await;
        Ok(())
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

/// Writer loop for coordinating write operations
///
/// All write operations (store, move, remove) go through this loop
/// to ensure ordering and atomicity.
async fn storage_writer_loop<S: StoreMail>(rx: Receiver<StorageCommand>, store: S) {
    while let Ok(cmd) = rx.recv().await {
        match cmd {
            StorageCommand::Write { path, content, reply } => {
                let result = store.write(&path, &content).await;
                let _ = reply.send(result);
            }
            StorageCommand::Move { from, to, reply } => {
                let result = store.move_file(&from, &to).await;
                let _ = reply.send(result);
            }
            StorageCommand::Remove { path, reply } => {
                let result = store.remove(&path).await;
                let _ = reply.send(result);
            }
            StorageCommand::WriteMetadata { path, metadata, reply } => {
                let result = store.write_metadata(&path, &metadata).await;
                let _ = reply.send(result);
            }
            StorageCommand::Shutdown { reply } => {
                let _ = reply.send(());
                break;
            }
        }
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
        let storage = Storage::new(store, TestFormat).await.unwrap();
        storage.shutdown().await.unwrap();
    }

    #[async_std::test]
    async fn test_storage_store_retrieve() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemStore::new(tmp_dir.path()).await.unwrap();
        let storage = Storage::new(store, TestFormat).await.unwrap();

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
        let storage = Storage::new(store, TestFormat).await.unwrap();

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
        let storage = Storage::new(store, TestFormat).await.unwrap();

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
        let storage1 = Storage::new(store, TestFormat).await.unwrap();
        let storage2 = storage1.clone(); // Cheap clone!

        storage1.store("test1.eml", b"from storage1").await.unwrap();
        storage2.store("test2.eml", b"from storage2").await.unwrap();

        // Both should be able to read each other's writes
        assert!(storage1.exists("test2.eml").await.unwrap());
        assert!(storage2.exists("test1.eml").await.unwrap());

        storage1.shutdown().await.unwrap();
    }
}
