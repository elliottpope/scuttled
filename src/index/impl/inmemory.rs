//! In-memory index implementation

use async_std::channel::{bounded, Receiver, Sender};
use async_std::sync::{Arc, RwLock};
use async_std::task;
use async_trait::async_trait;
use futures::channel::oneshot;
use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::index::{Index, IndexedMessage};
use crate::types::*;

/// Write commands for the index (only writes go through the channel)
enum WriteCommand {
    CreateMailbox(String, String, oneshot::Sender<Result<()>>),
    DeleteMailbox(String, String, oneshot::Sender<Result<()>>),
    RenameMailbox(String, String, String, oneshot::Sender<Result<()>>),
    AddMessage(String, String, IndexedMessage, oneshot::Sender<Result<String>>),
    UpdateFlags(MessageId, Vec<MessageFlag>, oneshot::Sender<Result<()>>),
    DeleteMessage(MessageId, oneshot::Sender<Result<()>>),
    GetNextUid(String, String, oneshot::Sender<Result<Uid>>),
    Shutdown(oneshot::Sender<()>),
}

/// State for the index
struct IndexState {
    mailboxes: HashMap<String, MailboxState>,
    messages: HashMap<MessageId, MessageEntry>,
}

struct MailboxState {
    info: Mailbox,
    uid_counter: Uid,
}

struct MessageEntry {
    metadata: IndexedMessage,
    path: String,
    username: String,
}

/// In-memory index
///
/// Uses a channel-based writer loop for write operations to ensure ordering,
/// while read operations access the state directly via RwLock.
pub struct InMemoryIndex {
    state: Arc<RwLock<IndexState>>,
    write_tx: Sender<WriteCommand>,
}

impl InMemoryIndex {
    pub fn new() -> Self {
        let state = Arc::new(RwLock::new(IndexState {
            mailboxes: HashMap::new(),
            messages: HashMap::new(),
        }));

        let (write_tx, write_rx) = bounded(100);

        let state_clone = Arc::clone(&state);
        task::spawn(writer_loop(write_rx, state_clone));

        Self { state, write_tx }
    }

    fn make_mailbox_key(username: &str, mailbox: &str) -> String {
        format!("{}:{}", username, mailbox)
    }

    fn make_message_path(username: &str, mailbox: &str, message_id: &MessageId) -> String {
        format!("{}/{}/{}.eml", username, mailbox, message_id.0)
    }
}

impl Default for InMemoryIndex {
    fn default() -> Self {
        Self::new()
    }
}

async fn writer_loop(rx: Receiver<WriteCommand>, state: Arc<RwLock<IndexState>>) {
    while let Ok(cmd) = rx.recv().await {
        match cmd {
            WriteCommand::CreateMailbox(username, name, reply) => {
                let mut state = state.write().await;
                let result = create_mailbox(&mut state, &username, &name);
                let _ = reply.send(result);
            }
            WriteCommand::DeleteMailbox(username, name, reply) => {
                let mut state = state.write().await;
                let result = delete_mailbox(&mut state, &username, &name);
                let _ = reply.send(result);
            }
            WriteCommand::RenameMailbox(username, old_name, new_name, reply) => {
                let mut state = state.write().await;
                let result = rename_mailbox(&mut state, &username, &old_name, &new_name);
                let _ = reply.send(result);
            }
            WriteCommand::AddMessage(username, mailbox, message, reply) => {
                let mut state = state.write().await;
                let result = add_message(&mut state, &username, &mailbox, message);
                let _ = reply.send(result);
            }
            WriteCommand::UpdateFlags(id, flags, reply) => {
                let mut state = state.write().await;
                let result = update_flags(&mut state, id, flags);
                let _ = reply.send(result);
            }
            WriteCommand::DeleteMessage(id, reply) => {
                let mut state = state.write().await;
                let result = delete_message(&mut state, id);
                let _ = reply.send(result);
            }
            WriteCommand::GetNextUid(username, mailbox, reply) => {
                let mut state = state.write().await;
                let result = get_next_uid(&mut state, &username, &mailbox);
                let _ = reply.send(result);
            }
            WriteCommand::Shutdown(reply) => {
                let _ = reply.send(());
                break;
            }
        }
    }
}

fn create_mailbox(state: &mut IndexState, username: &str, name: &str) -> Result<()> {
    let key = InMemoryIndex::make_mailbox_key(username, name);

    if state.mailboxes.contains_key(&key) {
        return Err(Error::AlreadyExists(format!("Mailbox {} already exists", name)));
    }

    state.mailboxes.insert(
        key,
        MailboxState {
            info: Mailbox {
                name: name.to_string(),
                uid_validity: 1,
                uid_next: 1,
                flags: vec![],
                permanent_flags: vec![MessageFlag::Seen, MessageFlag::Answered, MessageFlag::Flagged, MessageFlag::Deleted, MessageFlag::Draft],
                message_count: 0,
                recent_count: 0,
                unseen_count: 0,
            },
            uid_counter: 1,
        },
    );

    Ok(())
}

fn delete_mailbox(state: &mut IndexState, username: &str, name: &str) -> Result<()> {
    let key = InMemoryIndex::make_mailbox_key(username, name);

    if !state.mailboxes.contains_key(&key) {
        return Err(Error::NotFound(format!("Mailbox {} not found", name)));
    }

    // Remove all messages in the mailbox
    state.messages.retain(|_, entry| {
        !(entry.username == username && entry.metadata.mailbox == name)
    });

    state.mailboxes.remove(&key);
    Ok(())
}

fn rename_mailbox(state: &mut IndexState, username: &str, old_name: &str, new_name: &str) -> Result<()> {
    let old_key = InMemoryIndex::make_mailbox_key(username, old_name);
    let new_key = InMemoryIndex::make_mailbox_key(username, new_name);

    if !state.mailboxes.contains_key(&old_key) {
        return Err(Error::NotFound(format!("Mailbox {} not found", old_name)));
    }

    if state.mailboxes.contains_key(&new_key) {
        return Err(Error::AlreadyExists(format!("Mailbox {} already exists", new_name)));
    }

    if let Some(mut mailbox_state) = state.mailboxes.remove(&old_key) {
        mailbox_state.info.name = new_name.to_string();
        state.mailboxes.insert(new_key, mailbox_state);

        // Update message entries
        for entry in state.messages.values_mut() {
            if entry.username == username && entry.metadata.mailbox == old_name {
                entry.metadata.mailbox = new_name.to_string();
                entry.path = InMemoryIndex::make_message_path(username, new_name, &entry.metadata.id);
            }
        }
    }

    Ok(())
}

fn list_mailboxes(state: &IndexState, username: &str) -> Result<Vec<Mailbox>> {
    let prefix = format!("{}:", username);
    let mailboxes = state
        .mailboxes
        .iter()
        .filter_map(|(key, mailbox_state)| {
            if key.starts_with(&prefix) {
                Some(mailbox_state.info.clone())
            } else {
                None
            }
        })
        .collect();
    Ok(mailboxes)
}

fn get_mailbox(state: &IndexState, username: &str, name: &str) -> Result<Option<Mailbox>> {
    let key = InMemoryIndex::make_mailbox_key(username, name);
    Ok(state.mailboxes.get(&key).map(|ms| ms.info.clone()))
}

fn add_message(state: &mut IndexState, username: &str, mailbox: &str, message: IndexedMessage) -> Result<String> {
    let key = InMemoryIndex::make_mailbox_key(username, mailbox);

    if !state.mailboxes.contains_key(&key) {
        return Err(Error::NotFound(format!("Mailbox {} not found", mailbox)));
    }

    let path = InMemoryIndex::make_message_path(username, mailbox, &message.id);

    state.messages.insert(
        message.id,
        MessageEntry {
            metadata: message.clone(),
            path: path.clone(),
            username: username.to_string(),
        },
    );

    // Update mailbox counts
    if let Some(mailbox_state) = state.mailboxes.get_mut(&key) {
        mailbox_state.info.message_count += 1;
        if !message.flags.contains(&MessageFlag::Seen) {
            mailbox_state.info.unseen_count += 1;
        }
    }

    Ok(path)
}

fn get_message_path(state: &IndexState, id: MessageId) -> Result<Option<String>> {
    Ok(state.messages.get(&id).map(|entry| entry.path.clone()))
}

fn get_message_path_by_uid(state: &IndexState, username: &str, mailbox: &str, uid: Uid) -> Result<Option<String>> {
    for entry in state.messages.values() {
        if entry.username == username
            && entry.metadata.mailbox == mailbox
            && entry.metadata.uid == uid {
            return Ok(Some(entry.path.clone()));
        }
    }
    Ok(None)
}

fn list_message_paths(state: &IndexState, username: &str, mailbox: &str) -> Result<Vec<String>> {
    let paths = state
        .messages
        .values()
        .filter(|entry| entry.username == username && entry.metadata.mailbox == mailbox)
        .map(|entry| entry.path.clone())
        .collect();
    Ok(paths)
}

fn get_message_metadata(state: &IndexState, id: MessageId) -> Result<Option<IndexedMessage>> {
    Ok(state.messages.get(&id).map(|entry| entry.metadata.clone()))
}

fn update_flags(state: &mut IndexState, id: MessageId, flags: Vec<MessageFlag>) -> Result<()> {
    if let Some(entry) = state.messages.get_mut(&id) {
        entry.metadata.flags = flags;
        Ok(())
    } else {
        Err(Error::NotFound(format!("Message {:?} not found", id)))
    }
}

fn delete_message(state: &mut IndexState, id: MessageId) -> Result<()> {
    if let Some(entry) = state.messages.remove(&id) {
        let key = InMemoryIndex::make_mailbox_key(&entry.username, &entry.metadata.mailbox);
        if let Some(mailbox_state) = state.mailboxes.get_mut(&key) {
            mailbox_state.info.message_count = mailbox_state.info.message_count.saturating_sub(1);
            if !entry.metadata.flags.contains(&MessageFlag::Seen) {
                mailbox_state.info.unseen_count = mailbox_state.info.unseen_count.saturating_sub(1);
            }
        }
        Ok(())
    } else {
        Err(Error::NotFound(format!("Message {:?} not found", id)))
    }
}

fn search(state: &IndexState, username: &str, mailbox: &str, query: &SearchQuery) -> Result<Vec<String>> {
    let results = state
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

fn get_next_uid(state: &mut IndexState, username: &str, mailbox: &str) -> Result<Uid> {
    let key = InMemoryIndex::make_mailbox_key(username, mailbox);

    if let Some(mailbox_state) = state.mailboxes.get_mut(&key) {
        let uid = mailbox_state.uid_counter;
        mailbox_state.uid_counter += 1;
        Ok(uid)
    } else {
        Err(Error::NotFound(format!("Mailbox {} not found", mailbox)))
    }
}

#[async_trait]
impl Index for InMemoryIndex {
    async fn create_mailbox(&self, username: &str, name: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::CreateMailbox(username.to_string(), name.to_string(), tx))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    async fn delete_mailbox(&self, username: &str, name: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::DeleteMailbox(username.to_string(), name.to_string(), tx))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    async fn rename_mailbox(&self, username: &str, old_name: &str, new_name: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::RenameMailbox(
                username.to_string(),
                old_name.to_string(),
                new_name.to_string(),
                tx,
            ))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    async fn list_mailboxes(&self, username: &str) -> Result<Vec<Mailbox>> {
        // Direct read - no channel needed
        let state = self.state.read().await;
        list_mailboxes(&state, username)
    }

    async fn get_mailbox(&self, username: &str, name: &str) -> Result<Option<Mailbox>> {
        // Direct read - no channel needed
        let state = self.state.read().await;
        get_mailbox(&state, username, name)
    }

    async fn add_message(&self, username: &str, mailbox: &str, message: IndexedMessage) -> Result<String> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::AddMessage(username.to_string(), mailbox.to_string(), message, tx))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    async fn get_message_path(&self, id: MessageId) -> Result<Option<String>> {
        // Direct read - no channel needed
        let state = self.state.read().await;
        get_message_path(&state, id)
    }

    async fn get_message_path_by_uid(&self, username: &str, mailbox: &str, uid: Uid) -> Result<Option<String>> {
        // Direct read - no channel needed
        let state = self.state.read().await;
        get_message_path_by_uid(&state, username, mailbox, uid)
    }

    async fn list_message_paths(&self, username: &str, mailbox: &str) -> Result<Vec<String>> {
        // Direct read - no channel needed
        let state = self.state.read().await;
        list_message_paths(&state, username, mailbox)
    }

    async fn get_message_metadata(&self, id: MessageId) -> Result<Option<IndexedMessage>> {
        // Direct read - no channel needed
        let state = self.state.read().await;
        get_message_metadata(&state, id)
    }

    async fn update_flags(&self, id: MessageId, flags: Vec<MessageFlag>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::UpdateFlags(id, flags, tx))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    async fn delete_message(&self, id: MessageId) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::DeleteMessage(id, tx))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    async fn search(&self, username: &str, mailbox: &str, query: &SearchQuery) -> Result<Vec<String>> {
        // Direct read - no channel needed
        let state = self.state.read().await;
        search(&state, username, mailbox, query)
    }

    async fn get_next_uid(&self, username: &str, mailbox: &str) -> Result<Uid> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::GetNextUid(username.to_string(), mailbox.to_string(), tx))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
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

    #[async_std::test]
    async fn test_create_and_list_mailboxes() {
        let index = InMemoryIndex::new();

        index.create_mailbox("alice", "INBOX").await.unwrap();
        index.create_mailbox("alice", "Sent").await.unwrap();

        let mailboxes = index.list_mailboxes("alice").await.unwrap();
        assert_eq!(mailboxes.len(), 2);
    }

    #[async_std::test]
    async fn test_add_and_retrieve_message() {
        let index = InMemoryIndex::new();

        index.create_mailbox("alice", "INBOX").await.unwrap();

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

        let path = index.add_message("alice", "INBOX", message.clone()).await.unwrap();
        assert!(path.contains("alice/INBOX"));

        let retrieved = index.get_message_path(message.id).await.unwrap();
        assert_eq!(retrieved, Some(path));
    }

    #[async_std::test]
    async fn test_search() {
        let index = InMemoryIndex::new();
        index.create_mailbox("alice", "INBOX").await.unwrap();

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

        let results = index.search("alice", "INBOX", &SearchQuery::Subject("Important".to_string())).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[async_std::test]
    async fn test_delete_message() {
        let index = InMemoryIndex::new();
        index.create_mailbox("alice", "INBOX").await.unwrap();

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

        index.add_message("alice", "INBOX", message.clone()).await.unwrap();
        index.delete_message(message.id).await.unwrap();

        let retrieved = index.get_message_path(message.id).await.unwrap();
        assert_eq!(retrieved, None);
    }

    #[async_std::test]
    async fn test_rename_mailbox() {
        let index = InMemoryIndex::new();
        index.create_mailbox("alice", "Drafts").await.unwrap();

        index.rename_mailbox("alice", "Drafts", "MyDrafts").await.unwrap();

        let mailbox = index.get_mailbox("alice", "MyDrafts").await.unwrap();
        assert!(mailbox.is_some());

        let old_mailbox = index.get_mailbox("alice", "Drafts").await.unwrap();
        assert!(old_mailbox.is_none());
    }
}
