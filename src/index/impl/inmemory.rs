//! In-memory index backend implementation
//!
//! This backend stores all index data in memory using HashMaps.
//! It's simple, fast, and useful for testing, but data is lost on restart.

use async_trait::async_trait;
use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::index::backend::IndexBackend;
use crate::index::{IndexedMessage, Indexer};
use crate::types::*;

/// State for a single mailbox
struct MailboxState {
    uid_counter: Uid,
}

/// Message entry with metadata and path
struct MessageEntry {
    metadata: IndexedMessage,
    path: String,
    username: String,
}

/// In-memory storage backend for the index
///
/// This is a simple storage implementation that keeps all data in memory.
/// It's focused purely on storage operations - EventBus integration and
/// write ordering are handled by the Indexer wrapper.
pub struct InMemoryBackend {
    /// Mailbox state (uid counters) keyed by "username:mailbox"
    mailboxes: HashMap<String, MailboxState>,
    /// Messages keyed by MessageId
    messages: HashMap<MessageId, MessageEntry>,
}

impl InMemoryBackend {
    /// Create a new InMemoryBackend
    pub fn new() -> Self {
        Self {
            mailboxes: HashMap::new(),
            messages: HashMap::new(),
        }
    }

    fn make_mailbox_key(username: &str, mailbox: &str) -> String {
        format!("{}:{}", username, mailbox)
    }

    fn make_message_path(username: &str, mailbox: &str, message_id: &MessageId) -> String {
        format!("{}/{}/{}.eml", username, mailbox, message_id.0)
    }
}

impl Default for InMemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl IndexBackend for InMemoryBackend {
    async fn initialize_mailbox(&mut self, username: &str, mailbox: &str) -> Result<()> {
        let key = Self::make_mailbox_key(username, mailbox);

        if self.mailboxes.contains_key(&key) {
            return Err(Error::AlreadyExists(format!(
                "Mailbox {} already initialized",
                mailbox
            )));
        }

        self.mailboxes.insert(
            key,
            MailboxState {
                uid_counter: 1,
            },
        );

        Ok(())
    }

    async fn remove_mailbox_data(&mut self, username: &str, mailbox: &str) -> Result<()> {
        let key = Self::make_mailbox_key(username, mailbox);

        if !self.mailboxes.contains_key(&key) {
            return Err(Error::NotFound(format!("Mailbox {} not found", mailbox)));
        }

        // Remove all messages in the mailbox
        self.messages.retain(|_, entry| {
            !(entry.username == username && entry.metadata.mailbox == mailbox)
        });

        self.mailboxes.remove(&key);
        Ok(())
    }

    async fn mailbox_exists(&self, username: &str, mailbox: &str) -> Result<bool> {
        let key = Self::make_mailbox_key(username, mailbox);
        Ok(self.mailboxes.contains_key(&key))
    }

    async fn add_message(
        &mut self,
        username: &str,
        mailbox: &str,
        message: IndexedMessage,
    ) -> Result<String> {
        let key = Self::make_mailbox_key(username, mailbox);

        if !self.mailboxes.contains_key(&key) {
            return Err(Error::NotFound(format!("Mailbox {} not found", mailbox)));
        }

        let path = Self::make_message_path(username, mailbox, &message.id);

        self.messages.insert(
            message.id,
            MessageEntry {
                metadata: message,
                path: path.clone(),
                username: username.to_string(),
            },
        );

        Ok(path)
    }

    async fn update_flags(&mut self, id: MessageId, flags: Vec<MessageFlag>) -> Result<()> {
        if let Some(entry) = self.messages.get_mut(&id) {
            entry.metadata.flags = flags;
            Ok(())
        } else {
            Err(Error::NotFound(format!("Message {:?} not found", id)))
        }
    }

    async fn delete_message(&mut self, id: MessageId) -> Result<()> {
        if self.messages.remove(&id).is_some() {
            Ok(())
        } else {
            Err(Error::NotFound(format!("Message {:?} not found", id)))
        }
    }

    async fn get_message_path(&self, id: MessageId) -> Result<Option<String>> {
        Ok(self.messages.get(&id).map(|entry| entry.path.clone()))
    }

    async fn get_message_path_by_uid(
        &self,
        username: &str,
        mailbox: &str,
        uid: Uid,
    ) -> Result<Option<String>> {
        for entry in self.messages.values() {
            if entry.username == username
                && entry.metadata.mailbox == mailbox
                && entry.metadata.uid == uid
            {
                return Ok(Some(entry.path.clone()));
            }
        }
        Ok(None)
    }

    async fn list_message_paths(&self, username: &str, mailbox: &str) -> Result<Vec<String>> {
        let paths = self
            .messages
            .values()
            .filter(|entry| entry.username == username && entry.metadata.mailbox == mailbox)
            .map(|entry| entry.path.clone())
            .collect();
        Ok(paths)
    }

    async fn get_message_metadata(&self, id: MessageId) -> Result<Option<IndexedMessage>> {
        Ok(self
            .messages
            .get(&id)
            .map(|entry| entry.metadata.clone()))
    }

    async fn search(
        &self,
        username: &str,
        mailbox: &str,
        query: &SearchQuery,
    ) -> Result<Vec<String>> {
        let results = self
            .messages
            .values()
            .filter(|entry| {
                entry.username == username
                    && entry.metadata.mailbox == mailbox
                    && matches_query(&entry.metadata, query)
            })
            .map(|entry| entry.path.clone())
            .collect();
        Ok(results)
    }
}

fn matches_query(message: &IndexedMessage, query: &SearchQuery) -> bool {
    match query {
        SearchQuery::All => true,
        SearchQuery::From(s) => message.from.contains(s),
        SearchQuery::To(s) => message.to.contains(s),
        SearchQuery::Subject(s) => message.subject.contains(s),
        SearchQuery::Body(s) => message.body_preview.contains(s),
        SearchQuery::Text(s) => {
            message.from.contains(s)
                || message.to.contains(s)
                || message.subject.contains(s)
                || message.body_preview.contains(s)
        }
        SearchQuery::Uid(uids) => uids.contains(&message.uid),
        SearchQuery::Sequence(_) => false, // Sequence numbers not tracked in index
        SearchQuery::Seen => message.flags.contains(&MessageFlag::Seen),
        SearchQuery::Unseen => !message.flags.contains(&MessageFlag::Seen),
        SearchQuery::Flagged => message.flags.contains(&MessageFlag::Flagged),
        SearchQuery::Unflagged => !message.flags.contains(&MessageFlag::Flagged),
        SearchQuery::Deleted => message.flags.contains(&MessageFlag::Deleted),
        SearchQuery::Undeleted => !message.flags.contains(&MessageFlag::Deleted),
        SearchQuery::And(q1, q2) => matches_query(message, q1) && matches_query(message, q2),
        SearchQuery::Or(q1, q2) => matches_query(message, q1) || matches_query(message, q2),
        SearchQuery::Not(q) => !matches_query(message, q),
    }
}

// Public type alias for backward compatibility
// Users should use this type, not InMemoryBackend directly
pub type InMemoryIndex = Indexer;

// Helper function to create an InMemoryIndex (Indexer with InMemoryBackend)
pub fn create_inmemory_index(
    mailboxes: Option<std::sync::Arc<dyn crate::mailboxes::Mailboxes>>,
) -> InMemoryIndex {
    Indexer::new(Box::new(InMemoryBackend::new()), mailboxes)
}

// Helper function to create an InMemoryIndex with EventBus integration
pub fn create_inmemory_index_with_eventbus(
    event_bus: std::sync::Arc<crate::events::EventBus>,
    mailboxes: Option<std::sync::Arc<dyn crate::mailboxes::Mailboxes>>,
) -> InMemoryIndex {
    Indexer::with_event_bus(Box::new(InMemoryBackend::new()), Some(event_bus), mailboxes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventBus;
    use async_std::sync::Arc;

    #[async_std::test]
    async fn test_create_and_initialize_mailbox() {
        let index = create_inmemory_index(None);

        index
            .initialize_mailbox("alice", "INBOX")
            .await
            .unwrap();
        index.initialize_mailbox("alice", "Sent").await.unwrap();

        assert!(index.mailbox_exists("alice", "INBOX").await.unwrap());
        assert!(index.mailbox_exists("alice", "Sent").await.unwrap());
    }

    #[async_std::test]
    async fn test_add_and_retrieve_message() {
        let index = create_inmemory_index(None);

        index
            .initialize_mailbox("alice", "INBOX")
            .await
            .unwrap();

        let message = IndexedMessage {
            id: MessageId::new(),
            uid: 1,
            mailbox: "INBOX".to_string(),
            flags: vec![],
            from: "bob@example.com".to_string(),
            to: "alice@example.com".to_string(),
            subject: "Test".to_string(),
            body_preview: "Test body".to_string(),
            internal_date: chrono::Utc::now(),
            size: 100,
        };

        let path = index
            .add_message("alice", "INBOX", message.clone())
            .await
            .unwrap();
        assert!(path.contains("alice/INBOX"));

        let retrieved = index.get_message_path(message.id).await.unwrap();
        assert_eq!(retrieved, Some(path));
    }

    #[async_std::test]
    async fn test_search() {
        let index = create_inmemory_index(None);
        index
            .initialize_mailbox("alice", "INBOX")
            .await
            .unwrap();

        let message = IndexedMessage {
            id: MessageId::new(),
            uid: 1,
            mailbox: "INBOX".to_string(),
            flags: vec![],
            from: "bob@example.com".to_string(),
            to: "alice@example.com".to_string(),
            subject: "Important".to_string(),
            body_preview: "Test body".to_string(),
            internal_date: chrono::Utc::now(),
            size: 100,
        };

        index.add_message("alice", "INBOX", message).await.unwrap();

        let results = index
            .search(
                "alice",
                "INBOX",
                &SearchQuery::Subject("Important".to_string()),
            )
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[async_std::test]
    async fn test_delete_message() {
        let index = create_inmemory_index(None);
        index
            .initialize_mailbox("alice", "INBOX")
            .await
            .unwrap();

        let message = IndexedMessage {
            id: MessageId::new(),
            uid: 1,
            mailbox: "INBOX".to_string(),
            flags: vec![],
            from: "bob@example.com".to_string(),
            to: "alice@example.com".to_string(),
            subject: "Test".to_string(),
            body_preview: "Test body".to_string(),
            internal_date: chrono::Utc::now(),
            size: 100,
        };

        index
            .add_message("alice", "INBOX", message.clone())
            .await
            .unwrap();
        index.delete_message(message.id).await.unwrap();

        let retrieved = index.get_message_path(message.id).await.unwrap();
        assert_eq!(retrieved, None);
    }

    #[async_std::test]
    async fn test_remove_mailbox_data() {
        let index = create_inmemory_index(None);
        index
            .initialize_mailbox("alice", "Drafts")
            .await
            .unwrap();

        index.remove_mailbox_data("alice", "Drafts").await.unwrap();

        assert!(!index.mailbox_exists("alice", "Drafts").await.unwrap());
    }

    #[async_std::test]
    async fn test_event_bus_integration() {
        use crate::events::Event;
        use crate::mailboxes::r#impl::memory::InMemoryMailboxes;
        use crate::mailboxes::Mailboxes;

        // Create EventBus, Mailboxes, and Index with event integration
        let event_bus = Arc::new(EventBus::new());
        let mailboxes = Arc::new(InMemoryMailboxes::new());
        let index = create_inmemory_index_with_eventbus(event_bus.clone(), Some(mailboxes.clone()));

        // Initialize mailbox in both index and mailboxes registry
        index
            .initialize_mailbox("alice", "INBOX")
            .await
            .unwrap();
        mailboxes
            .create_mailbox("alice", "INBOX")
            .await
            .unwrap();

        // Give the event listener a moment to subscribe
        async_std::task::sleep(std::time::Duration::from_millis(50)).await;

        // Publish a MessageCreated event
        event_bus
            .publish(Event::MessageCreated {
                username: "alice".to_string(),
                mailbox: "INBOX".to_string(),
                unique_id: "1234567890.12345.test".to_string(),
                path: "alice/INBOX/1234567890.12345.test".to_string(),
                flags: vec![MessageFlag::Seen],
                is_new: false,
                from: "bob@example.com".to_string(),
                to: "alice@example.com".to_string(),
                subject: "Test from EventBus".to_string(),
                body_preview: "This message was created via EventBus".to_string(),
                size: 150,
                internal_date: chrono::Utc::now(),
            })
            .await
            .unwrap();

        // Give the index time to process the event
        async_std::task::sleep(std::time::Duration::from_millis(100)).await;

        // Verify the message was added to the index
        let messages = index.list_message_paths("alice", "INBOX").await.unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("alice/INBOX"));

        // Test MessageModified event
        event_bus
            .publish(Event::MessageModified {
                username: "alice".to_string(),
                mailbox: "INBOX".to_string(),
                unique_id: "1234567890.12345.test".to_string(),
                flags: vec![MessageFlag::Seen, MessageFlag::Flagged],
            })
            .await
            .unwrap();

        // Give the index time to process the event
        async_std::task::sleep(std::time::Duration::from_millis(100)).await;

        // Test MessageDeleted event
        event_bus
            .publish(Event::MessageDeleted {
                username: "alice".to_string(),
                mailbox: "INBOX".to_string(),
                unique_id: "1234567890.12345.test".to_string(),
            })
            .await
            .unwrap();

        // Give the index time to process the event
        async_std::task::sleep(std::time::Duration::from_millis(100)).await;

        // Verify the message was removed
        let messages = index.list_message_paths("alice", "INBOX").await.unwrap();
        assert_eq!(messages.len(), 0);
    }
}
