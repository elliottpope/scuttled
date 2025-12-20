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

/// Commands for the mailstore writer loop
enum Command {
    Store(String, Vec<u8>, oneshot::Sender<Result<()>>),
    Retrieve(String, oneshot::Sender<Result<Option<Vec<u8>>>>),
    Delete(String, oneshot::Sender<Result<()>>),
    Exists(String, oneshot::Sender<Result<bool>>),
    Shutdown(oneshot::Sender<()>),
}

/// Filesystem-based mail store with channel-based writer loop
pub struct FilesystemMailStore {
    tx: Sender<Command>,
}

impl FilesystemMailStore {
    pub async fn new<P: AsRef<Path>>(root_path: P) -> Result<Self> {
        let root_path = root_path.as_ref().to_path_buf();
        fs::create_dir_all(&root_path).await?;

        let (tx, rx) = bounded(100);

        task::spawn(writer_loop(rx, root_path));

        Ok(Self { tx })
    }
}

async fn writer_loop(rx: Receiver<Command>, root_path: PathBuf) {
    while let Ok(cmd) = rx.recv().await {
        match cmd {
            Command::Store(path, content, reply) => {
                let result = store_file(&root_path, &path, &content).await;
                let _ = reply.send(result);
            }
            Command::Retrieve(path, reply) => {
                let result = retrieve_file(&root_path, &path).await;
                let _ = reply.send(result);
            }
            Command::Delete(path, reply) => {
                let result = delete_file(&root_path, &path).await;
                let _ = reply.send(result);
            }
            Command::Exists(path, reply) => {
                let result = check_exists(&root_path, &path).await;
                let _ = reply.send(result);
            }
            Command::Shutdown(reply) => {
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

async fn retrieve_file(root: &Path, path: &str) -> Result<Option<Vec<u8>>> {
    let full_path = root.join(path);

    if !full_path.exists().await {
        return Ok(None);
    }

    let mut file = File::open(&full_path).await?;
    let mut content = Vec::new();
    file.read_to_end(&mut content).await?;

    Ok(Some(content))
}

async fn delete_file(root: &Path, path: &str) -> Result<()> {
    let full_path = root.join(path);

    if !full_path.exists().await {
        return Err(Error::NotFound(format!("File not found: {}", path)));
    }

    fs::remove_file(&full_path).await?;

    // Try to remove parent directories if they're empty
    if let Some(parent) = full_path.parent() {
        let _ = fs::remove_dir(parent).await; // Ignore errors (dir might not be empty)
    }

    Ok(())
}

async fn check_exists(root: &Path, path: &str) -> Result<bool> {
    let full_path = root.join(path);
    Ok(full_path.exists().await)
}

#[async_trait]
impl MailStore for FilesystemMailStore {
    async fn store(&self, path: &str, content: &[u8]) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::Store(path.to_string(), content.to_vec(), tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?
    }

    async fn retrieve(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::Retrieve(path.to_string(), tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::Delete(path.to_string(), tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::Exists(path.to_string(), tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?
    }

    async fn shutdown(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::Shutdown(tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?;
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
        let content = b"Test email content";

        store.store(path, content).await.unwrap();

        let retrieved = store.retrieve(path).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap(), content);
    }

    #[async_std::test]
    async fn test_retrieve_nonexistent() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

        let retrieved = store.retrieve("nonexistent.eml").await.unwrap();
        assert!(retrieved.is_none());
    }

    #[async_std::test]
    async fn test_delete() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

        let path = "test/message.eml";
        let content = b"Test email content";

        store.store(path, content).await.unwrap();
        assert!(store.exists(path).await.unwrap());

        store.delete(path).await.unwrap();
        assert!(!store.exists(path).await.unwrap());
    }

    #[async_std::test]
    async fn test_exists() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

        let path = "test/message.eml";
        assert!(!store.exists(path).await.unwrap());

        store.store(path, b"content").await.unwrap();
        assert!(store.exists(path).await.unwrap());
    }

    #[async_std::test]
    async fn test_nested_paths() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

        let path = "user/mailbox/subfolder/message.eml";
        let content = b"Test content";

        store.store(path, content).await.unwrap();

        let retrieved = store.retrieve(path).await.unwrap();
        assert_eq!(retrieved.unwrap(), content);
    }

    #[async_std::test]
    async fn test_shutdown() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

        store.store("test.eml", b"content").await.unwrap();

        store.shutdown().await.unwrap();

        // After shutdown, operations should fail
        let result = store.store("test2.eml", b"content").await;
        assert!(result.is_err());
    }
}
