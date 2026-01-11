//! Filesystem implementation of StoreMail trait
//!
//! Simple, direct filesystem operations without channels.
//! The Storage layer handles write coordination.

use tokio::fs::{self, File};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::path::{Path, PathBuf};
use async_trait::async_trait;

use crate::error::{Error, Result};
use crate::storage::store_mail::StoreMail;

/// Filesystem-based storage backend (cheap to clone)
///
/// Simple wrapper around a root path. All operations are direct filesystem I/O.
#[derive(Clone)]
pub struct FilesystemStore {
    root: PathBuf,
}

impl FilesystemStore {
    /// Create a new FilesystemStore
    pub async fn new<P: AsRef<Path>>(root_path: P) -> Result<Self> {
        let root_path = root_path.as_ref().to_path_buf();
        fs::create_dir_all(&root_path).await?;

        Ok(Self { root: root_path })
    }
}

#[async_trait]
impl StoreMail for FilesystemStore {
    async fn write(&self, path: &str, content: &[u8]) -> Result<()> {
        let full_path = self.root.join(path);

        // Create parent directories if needed
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let mut file = File::create(&full_path).await?;
        file.write_all(content).await?;
        file.sync_all().await?;
        Ok(())
    }

    async fn move_file(&self, from: &str, to: &str) -> Result<()> {
        let from_path = self.root.join(from);
        let to_path = self.root.join(to);

        if !from_path.try_exists().unwrap_or(false) {
            return Err(Error::NotFound(format!("Source file not found: {}", from)));
        }

        // Create target directory if needed
        if let Some(parent) = to_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        fs::rename(&from_path, &to_path).await?;
        Ok(())
    }

    async fn remove(&self, path: &str) -> Result<()> {
        let full_path = self.root.join(path);

        if !full_path.try_exists().unwrap_or(false) {
            return Err(Error::NotFound(format!("File not found: {}", path)));
        }

        fs::remove_file(&full_path).await?;
        Ok(())
    }

    async fn write_metadata(&self, _path: &str, _metadata: &[u8]) -> Result<()> {
        // Metadata storage not implemented yet (future: xattrs)
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        let full_path = self.root.join(path);
        Ok(full_path.try_exists().unwrap_or(false))
    }

    async fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let full_path = self.root.join(path);

        if !full_path.try_exists().unwrap_or(false) {
            return Ok(None);
        }

        let mut file = File::open(&full_path).await?;
        let mut content = Vec::new();
        file.read_to_end(&mut content).await?;

        Ok(Some(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_filesystem_store_write_read() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemStore::new(tmp_dir.path()).await.unwrap();

        store.write("test.txt", b"Hello, World!").await.unwrap();

        let content = store.read("test.txt").await.unwrap();
        assert_eq!(content, Some(b"Hello, World!".to_vec()));
    }

    #[tokio::test]
    async fn test_filesystem_store_move() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemStore::new(tmp_dir.path()).await.unwrap();

        store.write("old.txt", b"content").await.unwrap();
        store.move_file("old.txt", "new.txt").await.unwrap();

        assert!(!store.exists("old.txt").await.unwrap());
        assert!(store.exists("new.txt").await.unwrap());
    }

    #[tokio::test]
    async fn test_filesystem_store_remove() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemStore::new(tmp_dir.path()).await.unwrap();

        store.write("test.txt", b"content").await.unwrap();
        assert!(store.exists("test.txt").await.unwrap());

        store.remove("test.txt").await.unwrap();
        assert!(!store.exists("test.txt").await.unwrap());
    }

    #[tokio::test]
    async fn test_filesystem_store_clone() {
        let tmp_dir = TempDir::new().unwrap();
        let store1 = FilesystemStore::new(tmp_dir.path()).await.unwrap();
        let store2 = store1.clone();

        store1.write("file1.txt", b"from store1").await.unwrap();
        store2.write("file2.txt", b"from store2").await.unwrap();

        assert!(store1.exists("file2.txt").await.unwrap());
        assert!(store2.exists("file1.txt").await.unwrap());
    }
}
