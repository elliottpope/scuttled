//! Storage layer for email messages
//!
//! This module provides a Copy/Clone storage handle for storing and retrieving
//! raw email messages. The Storage struct wraps a channel-based writer loop
//! for atomic write operations while allowing direct filesystem reads.
//!
//! # Architecture
//!
//! Storage is designed to be Copy/Clone to avoid Arc<dyn Trait> overhead:
//!
//! ```ignore
//! // Cheap to clone - just clones the channel sender
//! let storage = Storage::new("./data/mail").await?;
//! let storage_clone = storage.clone(); // No Arc needed!
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use scuttled::storage::Storage;
//!
//! let storage = Storage::new("./data/mail").await?;
//!
//! // Store a message
//! storage.store("alice/INBOX/msg1.eml", b"From: ...").await?;
//!
//! // Retrieve a message
//! let content = storage.retrieve("alice/INBOX/msg1.eml").await?;
//!
//! // Update flags (Maildir rename)
//! storage.update_flags("alice/INBOX/msg1.eml", &[MessageFlag::Seen]).await?;
//!
//! // Delete a message
//! storage.delete("alice/INBOX/msg1.eml").await?;
//! ```

use async_std::channel::{bounded, Receiver, Sender};
use async_std::fs::{self, File};
use async_std::io::ReadExt;
use async_std::path::{Path, PathBuf};
use async_std::task;
use futures::channel::oneshot;
use log::debug;

use crate::error::{Error, Result};
use crate::types::MessageFlag;

/// Write commands for the storage (only writes go through the channel)
#[derive(Debug)]
enum StorageCommand {
    Store {
        path: String,
        content: Vec<u8>,
        reply: oneshot::Sender<Result<()>>,
    },
    Delete {
        path: String,
        reply: oneshot::Sender<Result<()>>,
    },
    UpdateFlags {
        path: String,
        flags: Vec<MessageFlag>,
        reply: oneshot::Sender<Result<()>>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

/// Filesystem-based storage handle (cheap to clone)
///
/// This struct wraps a channel sender, making it cheap to clone and share
/// across components without Arc overhead.
#[derive(Clone)]
pub struct Storage {
    /// Root directory for storage
    root: PathBuf,
    /// Write command sender (cheap to clone)
    write_tx: Sender<StorageCommand>,
}

impl Storage {
    /// Create a new Storage instance
    ///
    /// This spawns a writer loop task and returns a handle that can be cloned cheaply.
    pub async fn new<P: AsRef<Path>>(root_path: P) -> Result<Self> {
        let root_path = root_path.as_ref().to_path_buf();
        fs::create_dir_all(&root_path).await?;

        let (write_tx, write_rx) = bounded(100);

        // Spawn writer loop for write operations
        let root_clone = root_path.clone();
        task::spawn(storage_writer_loop(write_rx, root_clone));

        debug!("Storage initialized at {:?}", root_path);

        Ok(Self {
            root: root_path,
            write_tx,
        })
    }

    /// Store a message at the given path
    pub async fn store(&self, path: &str, content: &[u8]) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(StorageCommand::Store {
                path: path.to_string(),
                content: content.to_vec(),
                reply: tx,
            })
            .await
            .map_err(|_| Error::Internal("Storage writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Storage writer loop dropped reply".to_string()))?
    }

    /// Retrieve a message from the given path
    pub async fn retrieve(&self, path: &str) -> Result<Option<Vec<u8>>> {
        // Direct filesystem read - no channel needed
        let full_path = self.root.join(path);

        if !full_path.exists().await {
            return Ok(None);
        }

        let mut file = File::open(&full_path).await?;
        let mut content = Vec::new();
        file.read_to_end(&mut content).await?;

        Ok(Some(content))
    }

    /// Delete a message at the given path
    pub async fn delete(&self, path: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(StorageCommand::Delete {
                path: path.to_string(),
                reply: tx,
            })
            .await
            .map_err(|_| Error::Internal("Storage writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Storage writer loop dropped reply".to_string()))?
    }

    /// Check if a message exists at the given path
    pub async fn exists(&self, path: &str) -> Result<bool> {
        // Direct filesystem check - no channel needed
        let full_path = self.root.join(path);
        Ok(full_path.exists().await)
    }

    /// Update flags for a message (Maildir rename)
    ///
    /// This renames the file to reflect new flags in the filename.
    /// For Maildir format: moves between new/cur and updates flags suffix.
    pub async fn update_flags(&self, path: &str, flags: &[MessageFlag]) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(StorageCommand::UpdateFlags {
                path: path.to_string(),
                flags: flags.to_vec(),
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
        let _ = self.write_tx.send(StorageCommand::Shutdown { reply: tx }).await;
        let _ = rx.await;
        Ok(())
    }

    /// Get the root path of the storage
    pub fn root_path(&self) -> &Path {
        &self.root
    }
}

/// Writer loop for storage operations
///
/// All write operations go through this loop to ensure ordering and atomicity.
async fn storage_writer_loop(rx: Receiver<StorageCommand>, root_path: PathBuf) {
    while let Ok(cmd) = rx.recv().await {
        match cmd {
            StorageCommand::Store { path, content, reply } => {
                let result = store_file(&root_path, &path, &content).await;
                let _ = reply.send(result);
            }
            StorageCommand::Delete { path, reply } => {
                let result = delete_file(&root_path, &path).await;
                let _ = reply.send(result);
            }
            StorageCommand::UpdateFlags { path, flags, reply } => {
                let result = update_flags_file(&root_path, &path, &flags).await;
                let _ = reply.send(result);
            }
            StorageCommand::Shutdown { reply } => {
                debug!("Storage writer loop shutting down");
                let _ = reply.send(());
                break;
            }
        }
    }
}

/// Store a file to disk
async fn store_file(root: &Path, path: &str, content: &[u8]) -> Result<()> {
    let full_path = root.join(path);

    // Create parent directories if they don't exist
    if let Some(parent) = full_path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let mut file = File::create(&full_path).await?;
    use async_std::io::WriteExt;
    file.write_all(content).await?;
    file.sync_all().await?; // Ensure data is written to disk
    Ok(())
}

/// Delete a file from disk
async fn delete_file(root: &Path, path: &str) -> Result<()> {
    let full_path = root.join(path);

    if !full_path.exists().await {
        return Err(Error::NotFound(format!("File not found: {}", path)));
    }

    fs::remove_file(&full_path).await?;
    Ok(())
}

/// Update flags for a file (Maildir rename)
///
/// Renames the file to include flag information in the filename.
/// Maildir format: filename:2,FLAGS where FLAGS are single letters:
/// - D = Draft
/// - F = Flagged
/// - P = Passed (forwarded)
/// - R = Replied
/// - S = Seen
/// - T = Trashed (deleted)
async fn update_flags_file(root: &Path, path: &str, flags: &[MessageFlag]) -> Result<()> {
    let full_path = root.join(path);

    if !full_path.exists().await {
        return Err(Error::NotFound(format!("File not found: {}", path)));
    }

    // Convert flags to Maildir flag string
    let flag_str = flags_to_maildir_string(flags);

    // Parse the current path to get components
    let path_obj = Path::new(path);
    let filename = path_obj
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::Internal(format!("Invalid path: {}", path)))?;

    // Extract unique ID (before :2, if present)
    let unique_id = if let Some(idx) = filename.find(":2,") {
        &filename[..idx]
    } else {
        filename
    };

    // Build new filename with flags
    let new_filename = if flag_str.is_empty() {
        unique_id.to_string()
    } else {
        format!("{}:2,{}", unique_id, flag_str)
    };

    // Determine if we need to move between new/cur directories
    let parent = path_obj.parent().unwrap_or(Path::new(""));
    let parent_name = parent
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    let new_parent = if flags.iter().any(|f| matches!(f, MessageFlag::Seen)) {
        // Seen messages go in cur/
        if parent_name == "new" {
            parent.parent().unwrap_or(parent).join("cur")
        } else {
            parent.to_path_buf()
        }
    } else {
        // Unseen messages go in new/
        if parent_name == "cur" {
            parent.parent().unwrap_or(parent).join("new")
        } else {
            parent.to_path_buf()
        }
    };

    let new_path = new_parent.join(new_filename);
    let new_full_path = root.join(&new_path);

    // Create target directory if needed
    if let Some(new_parent_full) = new_full_path.parent() {
        fs::create_dir_all(new_parent_full).await?;
    }

    // Rename the file
    fs::rename(&full_path, &new_full_path).await?;

    debug!("Updated flags: {:?} -> {:?}", path, new_path);
    Ok(())
}

/// Convert MessageFlags to Maildir flag string
///
/// Flags are alphabetically sorted as per Maildir spec.
fn flags_to_maildir_string(flags: &[MessageFlag]) -> String {
    let mut chars = Vec::new();

    for flag in flags {
        match flag {
            MessageFlag::Draft => chars.push('D'),
            MessageFlag::Flagged => chars.push('F'),
            MessageFlag::Answered => chars.push('R'), // Replied
            MessageFlag::Seen => chars.push('S'),
            MessageFlag::Deleted => chars.push('T'), // Trashed
            MessageFlag::Recent => {} // Not stored in filename (transient)
            MessageFlag::Custom(_) => {} // Custom flags not supported in filename
        }
    }

    // Sort alphabetically
    chars.sort_unstable();
    chars.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[async_std::test]
    async fn test_storage_creation() {
        let tmp_dir = TempDir::new().unwrap();
        let storage = Storage::new(tmp_dir.path()).await.unwrap();
        storage.shutdown().await.unwrap();
    }

    #[async_std::test]
    async fn test_store_and_retrieve() {
        let tmp_dir = TempDir::new().unwrap();
        let storage = Storage::new(tmp_dir.path()).await.unwrap();

        let content = b"Hello, World!";
        storage.store("test/msg.eml", content).await.unwrap();

        let retrieved = storage.retrieve("test/msg.eml").await.unwrap();
        assert_eq!(retrieved, Some(content.to_vec()));

        storage.shutdown().await.unwrap();
    }

    #[async_std::test]
    async fn test_delete() {
        let tmp_dir = TempDir::new().unwrap();
        let storage = Storage::new(tmp_dir.path()).await.unwrap();

        storage.store("test/msg.eml", b"content").await.unwrap();
        assert!(storage.exists("test/msg.eml").await.unwrap());

        storage.delete("test/msg.eml").await.unwrap();
        assert!(!storage.exists("test/msg.eml").await.unwrap());

        storage.shutdown().await.unwrap();
    }

    #[async_std::test]
    async fn test_update_flags() {
        let tmp_dir = TempDir::new().unwrap();
        let storage = Storage::new(tmp_dir.path()).await.unwrap();

        // Store message in new/ (unseen)
        storage
            .store("alice/INBOX/new/msg123", b"content")
            .await
            .unwrap();

        // Mark as seen - should move to cur/ with :2,S flag
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
    async fn test_clone() {
        let tmp_dir = TempDir::new().unwrap();
        let storage1 = Storage::new(tmp_dir.path()).await.unwrap();
        let storage2 = storage1.clone(); // Cheap clone!

        storage1.store("test1.eml", b"from storage1").await.unwrap();
        storage2.store("test2.eml", b"from storage2").await.unwrap();

        // Both should be able to read each other's writes
        assert!(storage1.exists("test2.eml").await.unwrap());
        assert!(storage2.exists("test1.eml").await.unwrap());

        storage1.shutdown().await.unwrap();
    }

    #[test]
    fn test_flags_to_maildir_string() {
        assert_eq!(flags_to_maildir_string(&[]), "");
        assert_eq!(flags_to_maildir_string(&[MessageFlag::Seen]), "S");
        assert_eq!(
            flags_to_maildir_string(&[MessageFlag::Seen, MessageFlag::Flagged]),
            "FS"
        );
        assert_eq!(
            flags_to_maildir_string(&[
                MessageFlag::Answered,
                MessageFlag::Seen,
                MessageFlag::Draft
            ]),
            "DRS"
        );
    }
}
