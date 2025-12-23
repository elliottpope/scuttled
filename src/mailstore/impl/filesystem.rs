//! Filesystem-based mail storage implementation

use async_std::channel::{bounded, Receiver, Sender};
use async_std::fs::{self, File};
use async_std::io::{ReadExt, WriteExt};
use async_std::path::{Path, PathBuf};
use async_std::task;
use async_trait::async_trait;
use futures::channel::oneshot;

use crate::error::{Error, Result};
use crate::mailstore::MailStore;

/// Write commands for the mailstore (only writes go through the channel)
enum WriteCommand {
    Store(String, Vec<u8>, oneshot::Sender<Result<()>>),
    Delete(String, oneshot::Sender<Result<()>>),
    Shutdown(oneshot::Sender<()>),
}

/// Filesystem-based mail store
///
/// Uses a channel-based writer loop for write operations (store, delete)
/// to ensure ordering, while read operations access the filesystem directly.
pub struct FilesystemMailStore {
    root: PathBuf,
    write_tx: Sender<WriteCommand>,
}

impl FilesystemMailStore {
    pub async fn new<P: AsRef<Path>>(root_path: P) -> Result<Self> {
        let root_path = root_path.as_ref().to_path_buf();
        fs::create_dir_all(&root_path).await?;

        let (write_tx, write_rx) = bounded(100);

        // Spawn writer loop for write operations
        let root_clone = root_path.clone();
        task::spawn(writer_loop(write_rx, root_clone));

        Ok(Self {
            root: root_path,
            write_tx,
        })
    }
}

async fn writer_loop(rx: Receiver<WriteCommand>, root_path: PathBuf) {
    while let Ok(cmd) = rx.recv().await {
        match cmd {
            WriteCommand::Store(path, content, reply) => {
                let result = store_file(&root_path, &path, &content).await;
                let _ = reply.send(result);
            }
            WriteCommand::Delete(path, reply) => {
                let result = delete_file(&root_path, &path).await;
                let _ = reply.send(result);
            }
            WriteCommand::Shutdown(reply) => {
                let _ = reply.send(());
                break;
            }
        }
    }
}

async fn store_file(root: &Path, path: &str, content: &[u8]) -> Result<()> {
    let full_path = root.join(path);

    // Create parent directories if they don't exist
    if let Some(parent) = full_path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let mut file = File::create(&full_path).await?;
    file.write_all(content).await?;
    Ok(())
}

async fn delete_file(root: &Path, path: &str) -> Result<()> {
    let full_path = root.join(path);

    if !full_path.exists().await {
        return Err(Error::NotFound(format!("File not found: {}", path)));
    }

    fs::remove_file(&full_path).await?;
    Ok(())
}

#[async_trait]
impl MailStore for FilesystemMailStore {
    async fn store(&self, path: &str, content: &[u8]) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::Store(path.to_string(), content.to_vec(), tx))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    async fn retrieve(&self, path: &str) -> Result<Option<Vec<u8>>> {
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

    async fn delete(&self, path: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::Delete(path.to_string(), tx))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        // Direct filesystem check - no channel needed
        let full_path = self.root.join(path);
        Ok(full_path.exists().await)
    }

    async fn shutdown(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let _ = self.write_tx.send(WriteCommand::Shutdown(tx)).await;
        let _ = rx.await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[async_std::test]
    async fn test_store_and_retrieve() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

        let path = "test/message.eml";
        let content = b"test content";

        store.store(path, content).await.unwrap();
        let retrieved = store.retrieve(path).await.unwrap();

        assert_eq!(retrieved, Some(content.to_vec()));
    }

    #[async_std::test]
    async fn test_retrieve_nonexistent() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

        let retrieved = store.retrieve("nonexistent.eml").await.unwrap();
        assert_eq!(retrieved, None);
    }

    #[async_std::test]
    async fn test_exists() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

        let path = "test/exists.eml";
        assert!(!store.exists(path).await.unwrap());

        store.store(path, b"content").await.unwrap();
        assert!(store.exists(path).await.unwrap());
    }

    #[async_std::test]
    async fn test_delete() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

        let path = "test/delete.eml";
        store.store(path, b"content").await.unwrap();

        store.delete(path).await.unwrap();
        assert!(!store.exists(path).await.unwrap());
    }

    #[async_std::test]
    async fn test_nested_paths() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

        let path = "user/mailbox/subfolder/message.eml";
        store.store(path, b"content").await.unwrap();

        let retrieved = store.retrieve(path).await.unwrap();
        assert_eq!(retrieved, Some(b"content".to_vec()));
    }

    #[async_std::test]
    async fn test_shutdown() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

        store.shutdown().await.unwrap();
    }
}
