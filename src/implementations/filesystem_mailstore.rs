//! Filesystem-based mail storage implementation

use async_std::fs::{self, File, OpenOptions};
use async_std::io::{ReadExt, WriteExt};
use async_std::path::{Path, PathBuf};
use async_std::prelude::*;
use async_std::sync::RwLock;
use async_trait::async_trait;
use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::traits::MailStore;
use crate::types::*;

/// Filesystem-based mail store
pub struct FilesystemMailStore {
    root_path: PathBuf,
    uid_counters: RwLock<HashMap<MailboxName, Uid>>,
    mailboxes: RwLock<HashMap<MailboxName, Mailbox>>,
}

impl FilesystemMailStore {
    /// Create a new filesystem mail store at the given path
    pub async fn new<P: AsRef<Path>>(root_path: P) -> Result<Self> {
        let root_path = root_path.as_ref().to_path_buf();
        fs::create_dir_all(&root_path).await?;

        let store = Self {
            root_path,
            uid_counters: RwLock::new(HashMap::new()),
            mailboxes: RwLock::new(HashMap::new()),
        };

        store.load_mailboxes().await?;
        Ok(store)
    }

    async fn load_mailboxes(&self) -> Result<()> {
        let mut mailboxes = self.mailboxes.write().await;
        let mut uid_counters = self.uid_counters.write().await;

        let mut entries = fs::read_dir(&self.root_path).await?;
        while let Some(entry) = entries.next().await {
            let entry = entry?;
            if entry.file_type().await?.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                let mailbox = Mailbox {
                    name: name.clone(),
                    uid_validity: 1,
                    uid_next: 1,
                    flags: vec![],
                    permanent_flags: vec![],
                    message_count: 0,
                    recent_count: 0,
                    unseen_count: 0,
                };
                mailboxes.insert(name.clone(), mailbox);
                uid_counters.insert(name, 1);
            }
        }

        Ok(())
    }

    fn mailbox_path(&self, mailbox: &str) -> PathBuf {
        self.root_path.join(mailbox)
    }

    fn message_path(&self, mailbox: &str, id: MessageId) -> PathBuf {
        self.mailbox_path(mailbox).join(format!("{}.eml", id.0))
    }

    fn metadata_path(&self, mailbox: &str, id: MessageId) -> PathBuf {
        self.mailbox_path(mailbox).join(format!("{}.json", id.0))
    }

    async fn save_message_metadata(&self, message: &Message) -> Result<()> {
        let metadata_path = self.metadata_path(&message.mailbox, message.id);
        let json = serde_json::to_string_pretty(message)
            .map_err(|e| Error::Internal(format!("Failed to serialize metadata: {}", e)))?;

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&metadata_path)
            .await?;

        file.write_all(json.as_bytes()).await?;
        Ok(())
    }

    async fn load_message_metadata(&self, mailbox: &str, id: MessageId) -> Result<Option<Message>> {
        let metadata_path = self.metadata_path(mailbox, id);

        if !metadata_path.exists().await {
            return Ok(None);
        }

        let mut file = File::open(&metadata_path).await?;
        let mut contents = String::new();
        file.read_to_string(&mut contents).await?;

        let message: Message = serde_json::from_str(&contents)
            .map_err(|e| Error::Internal(format!("Failed to deserialize metadata: {}", e)))?;

        Ok(Some(message))
    }
}

#[async_trait]
impl MailStore for FilesystemMailStore {
    async fn store_message(&self, mailbox: &str, message: &Message) -> Result<()> {
        let mailbox_path = self.mailbox_path(mailbox);
        fs::create_dir_all(&mailbox_path).await?;

        let message_path = self.message_path(mailbox, message.id);
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&message_path)
            .await?;

        file.write_all(&message.raw_content).await?;

        self.save_message_metadata(message).await?;

        let mut mailboxes = self.mailboxes.write().await;
        if let Some(mb) = mailboxes.get_mut(mailbox) {
            mb.message_count += 1;
        }

        Ok(())
    }

    async fn get_message(&self, id: MessageId) -> Result<Option<Message>> {
        let mailboxes = self.mailboxes.read().await;
        for mailbox_name in mailboxes.keys() {
            if let Some(message) = self.load_message_metadata(mailbox_name, id).await? {
                return Ok(Some(message));
            }
        }
        Ok(None)
    }

    async fn get_message_by_uid(&self, mailbox: &str, uid: Uid) -> Result<Option<Message>> {
        let mailbox_path = self.mailbox_path(mailbox);
        if !mailbox_path.exists().await {
            return Ok(None);
        }

        let mut entries = fs::read_dir(&mailbox_path).await?;
        while let Some(entry) = entries.next().await {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                let mut file = File::open(&path).await?;
                let mut contents = String::new();
                file.read_to_string(&mut contents).await?;

                if let Ok(message) = serde_json::from_str::<Message>(&contents) {
                    if message.uid == uid {
                        return Ok(Some(message));
                    }
                }
            }
        }

        Ok(None)
    }

    async fn list_messages(&self, mailbox: &str) -> Result<Vec<Message>> {
        let mailbox_path = self.mailbox_path(mailbox);
        if !mailbox_path.exists().await {
            return Ok(Vec::new());
        }

        let mut messages = Vec::new();
        let mut entries = fs::read_dir(&mailbox_path).await?;

        while let Some(entry) = entries.next().await {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                let mut file = File::open(&path).await?;
                let mut contents = String::new();
                file.read_to_string(&mut contents).await?;

                if let Ok(message) = serde_json::from_str::<Message>(&contents) {
                    messages.push(message);
                }
            }
        }

        messages.sort_by_key(|m| m.uid);
        Ok(messages)
    }

    async fn delete_message(&self, id: MessageId) -> Result<()> {
        if let Some(message) = self.get_message(id).await? {
            let message_path = self.message_path(&message.mailbox, id);
            let metadata_path = self.metadata_path(&message.mailbox, id);

            if message_path.exists().await {
                fs::remove_file(message_path).await?;
            }
            if metadata_path.exists().await {
                fs::remove_file(metadata_path).await?;
            }

            let mut mailboxes = self.mailboxes.write().await;
            if let Some(mb) = mailboxes.get_mut(&message.mailbox) {
                mb.message_count = mb.message_count.saturating_sub(1);
            }

            Ok(())
        } else {
            Err(Error::NotFound(format!("Message not found: {:?}", id)))
        }
    }

    async fn update_flags(&self, id: MessageId, flags: Vec<MessageFlag>) -> Result<()> {
        if let Some(mut message) = self.get_message(id).await? {
            message.flags = flags;
            self.save_message_metadata(&message).await?;
            Ok(())
        } else {
            Err(Error::NotFound(format!("Message not found: {:?}", id)))
        }
    }

    async fn get_next_uid(&self, mailbox: &str) -> Result<Uid> {
        let mut uid_counters = self.uid_counters.write().await;
        let counter = uid_counters.entry(mailbox.to_string()).or_insert(1);
        let uid = *counter;
        *counter += 1;
        Ok(uid)
    }

    async fn create_mailbox(&self, name: &str) -> Result<()> {
        let mailbox_path = self.mailbox_path(name);
        if mailbox_path.exists().await {
            return Err(Error::AlreadyExists(format!("Mailbox already exists: {}", name)));
        }

        fs::create_dir_all(&mailbox_path).await?;

        let mut mailboxes = self.mailboxes.write().await;
        let mut uid_counters = self.uid_counters.write().await;

        mailboxes.insert(
            name.to_string(),
            Mailbox {
                name: name.to_string(),
                uid_validity: 1,
                uid_next: 1,
                flags: vec![],
                permanent_flags: vec![],
                message_count: 0,
                recent_count: 0,
                unseen_count: 0,
            },
        );
        uid_counters.insert(name.to_string(), 1);

        Ok(())
    }

    async fn delete_mailbox(&self, name: &str) -> Result<()> {
        let mailbox_path = self.mailbox_path(name);
        if !mailbox_path.exists().await {
            return Err(Error::NotFound(format!("Mailbox not found: {}", name)));
        }

        fs::remove_dir_all(&mailbox_path).await?;

        let mut mailboxes = self.mailboxes.write().await;
        let mut uid_counters = self.uid_counters.write().await;

        mailboxes.remove(name);
        uid_counters.remove(name);

        Ok(())
    }

    async fn list_mailboxes(&self, _username: &str) -> Result<Vec<Mailbox>> {
        let mailboxes = self.mailboxes.read().await;
        Ok(mailboxes.values().cloned().collect())
    }

    async fn get_mailbox(&self, name: &str) -> Result<Option<Mailbox>> {
        let mailboxes = self.mailboxes.read().await;
        Ok(mailboxes.get(name).cloned())
    }

    async fn rename_mailbox(&self, old_name: &str, new_name: &str) -> Result<()> {
        let old_path = self.mailbox_path(old_name);
        let new_path = self.mailbox_path(new_name);

        if !old_path.exists().await {
            return Err(Error::NotFound(format!("Mailbox not found: {}", old_name)));
        }

        if new_path.exists().await {
            return Err(Error::AlreadyExists(format!(
                "Mailbox already exists: {}",
                new_name
            )));
        }

        fs::rename(&old_path, &new_path).await?;

        let mut mailboxes = self.mailboxes.write().await;
        let mut uid_counters = self.uid_counters.write().await;

        if let Some(mut mailbox) = mailboxes.remove(old_name) {
            mailbox.name = new_name.to_string();
            mailboxes.insert(new_name.to_string(), mailbox);
        }

        if let Some(counter) = uid_counters.remove(old_name) {
            uid_counters.insert(new_name.to_string(), counter);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;

    #[async_std::test]
    async fn test_create_and_list_mailbox() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

        store.create_mailbox("INBOX").await.unwrap();
        let mailboxes = store.list_mailboxes("test_user").await.unwrap();

        assert_eq!(mailboxes.len(), 1);
        assert_eq!(mailboxes[0].name, "INBOX");
    }

    #[async_std::test]
    async fn test_store_and_retrieve_message() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

        store.create_mailbox("INBOX").await.unwrap();

        let message = Message {
            id: MessageId::new(),
            uid: 1,
            mailbox: "INBOX".to_string(),
            flags: vec![],
            internal_date: Utc::now(),
            size: 100,
            raw_content: b"Test email content".to_vec(),
        };

        store.store_message("INBOX", &message).await.unwrap();

        let retrieved = store.get_message(message.id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, message.id);
    }

    #[async_std::test]
    async fn test_delete_message() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

        store.create_mailbox("INBOX").await.unwrap();

        let message = Message {
            id: MessageId::new(),
            uid: 1,
            mailbox: "INBOX".to_string(),
            flags: vec![],
            internal_date: Utc::now(),
            size: 100,
            raw_content: b"Test email content".to_vec(),
        };

        store.store_message("INBOX", &message).await.unwrap();
        store.delete_message(message.id).await.unwrap();

        let retrieved = store.get_message(message.id).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[async_std::test]
    async fn test_update_flags() {
        let tmp_dir = TempDir::new().unwrap();
        let store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

        store.create_mailbox("INBOX").await.unwrap();

        let message = Message {
            id: MessageId::new(),
            uid: 1,
            mailbox: "INBOX".to_string(),
            flags: vec![],
            internal_date: Utc::now(),
            size: 100,
            raw_content: b"Test email content".to_vec(),
        };

        store.store_message("INBOX", &message).await.unwrap();

        let new_flags = vec![MessageFlag::Seen, MessageFlag::Flagged];
        store.update_flags(message.id, new_flags.clone()).await.unwrap();

        let retrieved = store.get_message(message.id).await.unwrap().unwrap();
        assert_eq!(retrieved.flags, new_flags);
    }
}
