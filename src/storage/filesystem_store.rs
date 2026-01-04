//! Filesystem implementation of StoreMail trait
//!
//! This implementation uses a channel-based writer loop for atomic write operations.

use async_std::channel::{bounded, Receiver, Sender};
use async_std::fs::{self, File};
use async_std::io::{ReadExt, WriteExt};
use async_std::path::{Path, PathBuf};
use async_std::task;
use async_trait::async_trait;
use futures::channel::oneshot;

use crate::error::{Error, Result};
use crate::storage::store_mail::StoreMail;

/// Commands for the filesystem storage backend
#[derive(Debug)]
enum StoreCommand {
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

/// Filesystem-based storage backend (cheap to clone)
#[derive(Clone)]
pub struct FilesystemStore {
    root: PathBuf,
    command_tx: Sender<StoreCommand>,
}

impl FilesystemStore {
    /// Create a new FilesystemStore
    pub async fn new<P: AsRef<Path>>(root_path: P) -> Result<Self> {
        let root_path = root_path.as_ref().to_path_buf();
        fs::create_dir_all(&root_path).await?;

        let (command_tx, command_rx) = bounded(100);

        // Spawn writer loop
        let root_clone = root_path.clone();
        task::spawn(store_writer_loop(command_rx, root_clone));

        Ok(Self {
            root: root_path,
            command_tx,
        })
    }
}

#[async_trait]
impl StoreMail for FilesystemStore {
    async fn write(&self, path: &str, content: &[u8]) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(StoreCommand::Write {
                path: path.to_string(),
                content: content.to_vec(),
                reply: tx,
            })
            .await
            .map_err(|_| Error::Internal("Store writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Store writer loop dropped reply".to_string()))?
    }

    async fn move_file(&self, from: &str, to: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(StoreCommand::Move {
                from: from.to_string(),
                to: to.to_string(),
                reply: tx,
            })
            .await
            .map_err(|_| Error::Internal("Store writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Store writer loop dropped reply".to_string()))?
    }

    async fn remove(&self, path: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(StoreCommand::Remove {
                path: path.to_string(),
                reply: tx,
            })
            .await
            .map_err(|_| Error::Internal("Store writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Store writer loop dropped reply".to_string()))?
    }

    async fn write_metadata(&self, path: &str, metadata: &[u8]) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(StoreCommand::WriteMetadata {
                path: path.to_string(),
                metadata: metadata.to_vec(),
                reply: tx,
            })
            .await
            .map_err(|_| Error::Internal("Store writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Store writer loop dropped reply".to_string()))?
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        // Direct filesystem check - no channel needed
        let full_path = self.root.join(path);
        Ok(full_path.exists().await)
    }

    async fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
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

    async fn shutdown(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let _ = self.command_tx.send(StoreCommand::Shutdown { reply: tx }).await;
        let _ = rx.await;
        Ok(())
    }
}

/// Writer loop for filesystem operations
async fn store_writer_loop(rx: Receiver<StoreCommand>, root: PathBuf) {
    while let Ok(cmd) = rx.recv().await {
        match cmd {
            StoreCommand::Write { path, content, reply } => {
                let result = write_file(&root, &path, &content).await;
                let _ = reply.send(result);
            }
            StoreCommand::Move { from, to, reply } => {
                let result = move_file(&root, &from, &to).await;
                let _ = reply.send(result);
            }
            StoreCommand::Remove { path, reply } => {
                let result = remove_file(&root, &path).await;
                let _ = reply.send(result);
            }
            StoreCommand::WriteMetadata { path: _, metadata: _, reply } => {
                // Metadata storage not implemented yet (future: xattrs)
                let _ = reply.send(Ok(()));
            }
            StoreCommand::Shutdown { reply } => {
                let _ = reply.send(());
                break;
            }
        }
    }
}

async fn write_file(root: &Path, path: &str, content: &[u8]) -> Result<()> {
    let full_path = root.join(path);

    // Create parent directories if needed
    if let Some(parent) = full_path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let mut file = File::create(&full_path).await?;
    file.write_all(content).await?;
    file.sync_all().await?;
    Ok(())
}

async fn move_file(root: &Path, from: &str, to: &str) -> Result<()> {
    let from_path = root.join(from);
    let to_path = root.join(to);

    if !from_path.exists().await {
        return Err(Error::NotFound(format!("Source file not found: {}", from)));
    }

    // Create target directory if needed
    if let Some(parent) = to_path.parent() {
        fs::create_dir_all(parent).await?;
    }

    fs::rename(&from_path, &to_path).await?;
    Ok(())
}

async fn remove_file(root: &Path, path: &str) -> Result<()> {
    let full_path = root.join(path);

    if !full_path.exists().await {
        return Err(Error::NotFound(format!("File not found: {}", path)));
    }

    fs::remove_file(&full_path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[async_std::test]
    async fn test_filesystem_store_write_read() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemStore::new(tmp_dir.path()).await.unwrap();

        store.write("test.txt", b"Hello, World!").await.unwrap();

        let content = store.read("test.txt").await.unwrap();
        assert_eq!(content, Some(b"Hello, World!".to_vec()));

        store.shutdown().await.unwrap();
    }

    #[async_std::test]
    async fn test_filesystem_store_move() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemStore::new(tmp_dir.path()).await.unwrap();

        store.write("old.txt", b"content").await.unwrap();
        store.move_file("old.txt", "new.txt").await.unwrap();

        assert!(!store.exists("old.txt").await.unwrap());
        assert!(store.exists("new.txt").await.unwrap());

        store.shutdown().await.unwrap();
    }

    #[async_std::test]
    async fn test_filesystem_store_remove() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemStore::new(tmp_dir.path()).await.unwrap();

        store.write("test.txt", b"content").await.unwrap();
        assert!(store.exists("test.txt").await.unwrap());

        store.remove("test.txt").await.unwrap();
        assert!(!store.exists("test.txt").await.unwrap());

        store.shutdown().await.unwrap();
    }

    #[async_std::test]
    async fn test_filesystem_store_clone() {
        let tmp_dir = TempDir::new().unwrap();
        let store1 = FilesystemStore::new(tmp_dir.path()).await.unwrap();
        let store2 = store1.clone();

        store1.write("file1.txt", b"from store1").await.unwrap();
        store2.write("file2.txt", b"from store2").await.unwrap();

        assert!(store1.exists("file2.txt").await.unwrap());
        assert!(store2.exists("file1.txt").await.unwrap());

        store1.shutdown().await.unwrap();
    }
}
