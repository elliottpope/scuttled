//! In-memory index implementation

use async_std::channel::{bounded, Receiver, Sender};
use async_std::task;
use async_trait::async_trait;
use chrono::Utc;
use futures::channel::oneshot;
use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::index::{Index, IndexedMessage};
use crate::types::*;

/// Commands for the index writer loop
enum Command {
    CreateMailbox(String, String, oneshot::Sender<Result<()>>),
    DeleteMailbox(String, String, oneshot::Sender<Result<()>>),
    RenameMailbox(String, String, String, oneshot::Sender<Result<()>>),
    ListMailboxes(String, oneshot::Sender<Result<Vec<Mailbox>>>),
    GetMailbox(String, String, oneshot::Sender<Result<Option<Mailbox>>>),
    AddMessage(String, String, IndexedMessage, oneshot::Sender<Result<String>>),
    GetMessagePath(MessageId, oneshot::Sender<Result<Option<String>>>),
    GetMessagePathByUid(String, String, Uid, oneshot::Sender<Result<Option<String>>>),
    ListMessagePaths(String, String, oneshot::Sender<Result<Vec<String>>>),
    GetMessageMetadata(MessageId, oneshot::Sender<Result<Option<IndexedMessage>>>),
    UpdateFlags(MessageId, Vec<MessageFlag>, oneshot::Sender<Result<()>>),
    DeleteMessage(MessageId, oneshot::Sender<Result<()>>),
    Search(String, String, SearchQuery, oneshot::Sender<Result<Vec<String>>>),
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

/// In-memory index with channel-based writer loop
pub struct InMemoryIndex {
    tx: Sender<Command>,
}

impl InMemoryIndex {
    pub fn new() -> Self {
        let (tx, rx) = bounded(100);

        task::spawn(writer_loop(rx));

        Self { tx }
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

async fn writer_loop(rx: Receiver<Command>) {
    let mut state = IndexState {
        mailboxes: HashMap::new(),
        messages: HashMap::new(),
    };

    while let Ok(cmd) = rx.recv().await {
        match cmd {
            Command::CreateMailbox(username, name, reply) => {
                let result = create_mailbox(&mut state, &username, &name);
                let _ = reply.send(result);
            }
            Command::DeleteMailbox(username, name, reply) => {
                let result = delete_mailbox(&mut state, &username, &name);
                let _ = reply.send(result);
            }
            Command::RenameMailbox(username, old_name, new_name, reply) => {
                let result = rename_mailbox(&mut state, &username, &old_name, &new_name);
                let _ = reply.send(result);
            }
            Command::ListMailboxes(username, reply) => {
                let result = list_mailboxes(&state, &username);
                let _ = reply.send(result);
            }
            Command::GetMailbox(username, name, reply) => {
                let result = get_mailbox(&state, &username, &name);
                let _ = reply.send(result);
            }
            Command::AddMessage(username, mailbox, message, reply) => {
                let result = add_message(&mut state, &username, &mailbox, message);
                let _ = reply.send(result);
            }
            Command::GetMessagePath(id, reply) => {
                let result = get_message_path(&state, id);
                let _ = reply.send(result);
            }
            Command::GetMessagePathByUid(username, mailbox, uid, reply) => {
                let result = get_message_path_by_uid(&state, &username, &mailbox, uid);
                let _ = reply.send(result);
            }
            Command::ListMessagePaths(username, mailbox, reply) => {
                let result = list_message_paths(&state, &username, &mailbox);
                let _ = reply.send(result);
            }
            Command::GetMessageMetadata(id, reply) => {
                let result = get_message_metadata(&state, id);
                let _ = reply.send(result);
            }
            Command::UpdateFlags(id, flags, reply) => {
                let result = update_flags(&mut state, id, flags);
                let _ = reply.send(result);
            }
            Command::DeleteMessage(id, reply) => {
                let result = delete_message(&mut state, id);
                let _ = reply.send(result);
            }
            Command::Search(username, mailbox, query, reply) => {
                let result = search(&state, &username, &mailbox, &query);
                let _ = reply.send(result);
            }
            Command::GetNextUid(username, mailbox, reply) => {
                let result = get_next_uid(&mut state, &username, &mailbox);
                let _ = reply.send(result);
            }
            Command::Shutdown(reply) => {
                let _ = reply.send(());
                break;
            }
        }
    }
}

fn create_mailbox(state: &mut IndexState, username: &str, name: &str) -> Result<()> {
    let key = InMemoryIndex::make_mailbox_key(username, name);

    if state.mailboxes.contains_key(&key) {
        return Err(Error::AlreadyExists(format!("Mailbox already exists: {}", name)));
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
        return Err(Error::NotFound(format!("Mailbox not found: {}", name)));
    }

    // Delete all messages in this mailbox
    let messages_to_delete: Vec<MessageId> = state
        .messages
        .iter()
        .filter(|(_, entry)| entry.username == username && entry.metadata.mailbox == name)
        .map(|(id, _)| *id)
        .collect();

    for id in messages_to_delete {
        state.messages.remove(&id);
    }

    state.mailboxes.remove(&key);

    Ok(())
}

fn rename_mailbox(state: &mut IndexState, username: &str, old_name: &str, new_name: &str) -> Result<()> {
    let old_key = InMemoryIndex::make_mailbox_key(username, old_name);
    let new_key = InMemoryIndex::make_mailbox_key(username, new_name);

    if !state.mailboxes.contains_key(&old_key) {
        return Err(Error::NotFound(format!("Mailbox not found: {}", old_name)));
    }

    if state.mailboxes.contains_key(&new_key) {
        return Err(Error::AlreadyExists(format!("Mailbox already exists: {}", new_name)));
    }

    // Update mailbox entry
    if let Some(mut mb_state) = state.mailboxes.remove(&old_key) {
        mb_state.info.name = new_name.to_string();
        state.mailboxes.insert(new_key, mb_state);
    }

    // Update all message entries for this mailbox
    for entry in state.messages.values_mut() {
        if entry.username == username && entry.metadata.mailbox == old_name {
            entry.metadata.mailbox = new_name.to_string();
            entry.path = InMemoryIndex::make_message_path(username, new_name, &entry.metadata.id);
        }
    }

    Ok(())
}

fn list_mailboxes(state: &IndexState, username: &str) -> Result<Vec<Mailbox>> {
    let prefix = format!("{}:", username);
    let mailboxes: Vec<Mailbox> = state
        .mailboxes
        .iter()
        .filter(|(key, _)| key.starts_with(&prefix))
        .map(|(_, mb_state)| mb_state.info.clone())
        .collect();

    Ok(mailboxes)
}

fn get_mailbox(state: &IndexState, username: &str, name: &str) -> Result<Option<Mailbox>> {
    let key = InMemoryIndex::make_mailbox_key(username, name);
    Ok(state.mailboxes.get(&key).map(|mb_state| mb_state.info.clone()))
}

fn add_message(state: &mut IndexState, username: &str, mailbox: &str, message: IndexedMessage) -> Result<String> {
    let key = InMemoryIndex::make_mailbox_key(username, mailbox);

    let mb_state = state
        .mailboxes
        .get_mut(&key)
        .ok_or_else(|| Error::InvalidMailbox(format!("Mailbox not found: {}", mailbox)))?;

    let path = InMemoryIndex::make_message_path(username, mailbox, &message.id);

    state.messages.insert(
        message.id,
        MessageEntry {
            metadata: message.clone(),
            path: path.clone(),
            username: username.to_string(),
        },
    );

    mb_state.info.message_count += 1;
    mb_state.info.uid_next = message.uid + 1;

    // Update unseen count
    if !message.flags.contains(&MessageFlag::Seen) {
        mb_state.info.unseen_count += 1;
    }

    Ok(path)
}

fn get_message_path(state: &IndexState, id: MessageId) -> Result<Option<String>> {
    Ok(state.messages.get(&id).map(|entry| entry.path.clone()))
}

fn get_message_path_by_uid(state: &IndexState, username: &str, mailbox: &str, uid: Uid) -> Result<Option<String>> {
    Ok(state
        .messages
        .values()
        .find(|entry| {
            entry.username == username
                && entry.metadata.mailbox == mailbox
                && entry.metadata.uid == uid
        })
        .map(|entry| entry.path.clone()))
}

fn list_message_paths(state: &IndexState, username: &str, mailbox: &str) -> Result<Vec<String>> {
    let mut paths: Vec<String> = state
        .messages
        .values()
        .filter(|entry| entry.username == username && entry.metadata.mailbox == mailbox)
        .map(|entry| entry.path.clone())
        .collect();

    paths.sort();
    Ok(paths)
}

fn get_message_metadata(state: &IndexState, id: MessageId) -> Result<Option<IndexedMessage>> {
    Ok(state.messages.get(&id).map(|entry| entry.metadata.clone()))
}

fn update_flags(state: &mut IndexState, id: MessageId, flags: Vec<MessageFlag>) -> Result<()> {
    let entry = state
        .messages
        .get_mut(&id)
        .ok_or_else(|| Error::NotFound(format!("Message not found: {:?}", id)))?;

    entry.metadata.flags = flags;

    Ok(())
}

fn delete_message(state: &mut IndexState, id: MessageId) -> Result<()> {
    if let Some(entry) = state.messages.remove(&id) {
        let key = InMemoryIndex::make_mailbox_key(&entry.username, &entry.metadata.mailbox);

        if let Some(mb_state) = state.mailboxes.get_mut(&key) {
            mb_state.info.message_count = mb_state.info.message_count.saturating_sub(1);

            if !entry.metadata.flags.contains(&MessageFlag::Seen) {
                mb_state.info.unseen_count = mb_state.info.unseen_count.saturating_sub(1);
            }
        }

        Ok(())
    } else {
        Err(Error::NotFound(format!("Message not found: {:?}", id)))
    }
}

fn search(state: &IndexState, username: &str, mailbox: &str, query: &SearchQuery) -> Result<Vec<String>> {
    let matching_paths: Vec<String> = state
        .messages
        .values()
        .filter(|entry| {
            entry.username == username
                && entry.metadata.mailbox == mailbox
                && matches_query(&entry.metadata, query)
        })
        .map(|entry| entry.path.clone())
        .collect();

    Ok(matching_paths)
}

fn matches_query(message: &IndexedMessage, query: &SearchQuery) -> bool {
    match query {
        SearchQuery::All => true,
        SearchQuery::Text(text) => {
            let text_lower = text.to_lowercase();
            message.from.to_lowercase().contains(&text_lower)
                || message.to.to_lowercase().contains(&text_lower)
                || message.subject.to_lowercase().contains(&text_lower)
                || message.body_preview.to_lowercase().contains(&text_lower)
        }
        SearchQuery::From(from) => message.from.to_lowercase().contains(&from.to_lowercase()),
        SearchQuery::To(to) => message.to.to_lowercase().contains(&to.to_lowercase()),
        SearchQuery::Subject(subject) => message.subject.to_lowercase().contains(&subject.to_lowercase()),
        SearchQuery::Body(body) => message.body_preview.to_lowercase().contains(&body.to_lowercase()),
        SearchQuery::Uid(uids) => uids.contains(&message.uid),
        SearchQuery::Seen => message.flags.contains(&MessageFlag::Seen),
        SearchQuery::Unseen => !message.flags.contains(&MessageFlag::Seen),
        SearchQuery::Flagged => message.flags.contains(&MessageFlag::Flagged),
        SearchQuery::Unflagged => !message.flags.contains(&MessageFlag::Flagged),
        SearchQuery::Deleted => message.flags.contains(&MessageFlag::Deleted),
        SearchQuery::Undeleted => !message.flags.contains(&MessageFlag::Deleted),
        SearchQuery::And(q1, q2) => matches_query(message, q1) && matches_query(message, q2),
        SearchQuery::Or(q1, q2) => matches_query(message, q1) || matches_query(message, q2),
        SearchQuery::Not(q) => !matches_query(message, q),
        _ => false,
    }
}

fn get_next_uid(state: &mut IndexState, username: &str, mailbox: &str) -> Result<Uid> {
    let key = InMemoryIndex::make_mailbox_key(username, mailbox);

    let mb_state = state
        .mailboxes
        .get_mut(&key)
        .ok_or_else(|| Error::InvalidMailbox(format!("Mailbox not found: {}", mailbox)))?;

    let uid = mb_state.uid_counter;
    mb_state.uid_counter += 1;

    Ok(uid)
}

#[async_trait]
impl Index for InMemoryIndex {
    async fn create_mailbox(&self, username: &str, name: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::CreateMailbox(username.to_string(), name.to_string(), tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?
    }

    async fn delete_mailbox(&self, username: &str, name: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::DeleteMailbox(username.to_string(), name.to_string(), tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?
    }

    async fn rename_mailbox(&self, username: &str, old_name: &str, new_name: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::RenameMailbox(username.to_string(), old_name.to_string(), new_name.to_string(), tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?
    }

    async fn list_mailboxes(&self, username: &str) -> Result<Vec<Mailbox>> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::ListMailboxes(username.to_string(), tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?
    }

    async fn get_mailbox(&self, username: &str, name: &str) -> Result<Option<Mailbox>> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::GetMailbox(username.to_string(), name.to_string(), tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?
    }

    async fn add_message(&self, username: &str, mailbox: &str, message: IndexedMessage) -> Result<String> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::AddMessage(username.to_string(), mailbox.to_string(), message, tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?
    }

    async fn get_message_path(&self, id: MessageId) -> Result<Option<String>> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::GetMessagePath(id, tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?
    }

    async fn get_message_path_by_uid(&self, username: &str, mailbox: &str, uid: Uid) -> Result<Option<String>> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::GetMessagePathByUid(username.to_string(), mailbox.to_string(), uid, tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?
    }

    async fn list_message_paths(&self, username: &str, mailbox: &str) -> Result<Vec<String>> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::ListMessagePaths(username.to_string(), mailbox.to_string(), tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?
    }

    async fn get_message_metadata(&self, id: MessageId) -> Result<Option<IndexedMessage>> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::GetMessageMetadata(id, tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?
    }

    async fn update_flags(&self, id: MessageId, flags: Vec<MessageFlag>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::UpdateFlags(id, flags, tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?
    }

    async fn delete_message(&self, id: MessageId) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::DeleteMessage(id, tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?
    }

    async fn search(&self, username: &str, mailbox: &str, query: &SearchQuery) -> Result<Vec<String>> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::Search(username.to_string(), mailbox.to_string(), query.clone(), tx)).await
            .map_err(|_| Error::Internal("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Reply channel closed".to_string()))?
    }

    async fn get_next_uid(&self, username: &str, mailbox: &str) -> Result<Uid> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::GetNextUid(username.to_string(), mailbox.to_string(), tx)).await
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

        let uid = index.get_next_uid("alice", "INBOX").await.unwrap();

        let message = IndexedMessage {
            id: MessageId::new(),
            uid,
            mailbox: "INBOX".to_string(),
            flags: vec![],
            internal_date: Utc::now(),
            size: 100,
            from: "bob@example.com".to_string(),
            to: "alice@example.com".to_string(),
            subject: "Hello".to_string(),
            body_preview: "Hello World".to_string(),
        };

        let path = index.add_message("alice", "INBOX", message.clone()).await.unwrap();

        assert!(path.contains("alice/INBOX"));

        let retrieved_path = index.get_message_path(message.id).await.unwrap();
        assert_eq!(retrieved_path, Some(path));
    }

    #[async_std::test]
    async fn test_search() {
        let index = InMemoryIndex::new();

        index.create_mailbox("alice", "INBOX").await.unwrap();

        let uid1 = index.get_next_uid("alice", "INBOX").await.unwrap();
        let msg1 = IndexedMessage {
            id: MessageId::new(),
            uid: uid1,
            mailbox: "INBOX".to_string(),
            flags: vec![],
            internal_date: Utc::now(),
            size: 100,
            from: "bob@example.com".to_string(),
            to: "alice@example.com".to_string(),
            subject: "Hello".to_string(),
            body_preview: "Hello World".to_string(),
        };

        let uid2 = index.get_next_uid("alice", "INBOX").await.unwrap();
        let msg2 = IndexedMessage {
            id: MessageId::new(),
            uid: uid2,
            mailbox: "INBOX".to_string(),
            flags: vec![MessageFlag::Seen],
            internal_date: Utc::now(),
            size: 100,
            from: "charlie@example.com".to_string(),
            to: "alice@example.com".to_string(),
            subject: "Meeting".to_string(),
            body_preview: "Let's meet".to_string(),
        };

        index.add_message("alice", "INBOX", msg1.clone()).await.unwrap();
        index.add_message("alice", "INBOX", msg2.clone()).await.unwrap();

        let results = index.search("alice", "INBOX", &SearchQuery::From("bob".to_string())).await.unwrap();
        assert_eq!(results.len(), 1);

        let results = index.search("alice", "INBOX", &SearchQuery::Unseen).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[async_std::test]
    async fn test_delete_message() {
        let index = InMemoryIndex::new();

        index.create_mailbox("alice", "INBOX").await.unwrap();

        let uid = index.get_next_uid("alice", "INBOX").await.unwrap();
        let message = IndexedMessage {
            id: MessageId::new(),
            uid,
            mailbox: "INBOX".to_string(),
            flags: vec![],
            internal_date: Utc::now(),
            size: 100,
            from: "bob@example.com".to_string(),
            to: "alice@example.com".to_string(),
            subject: "Hello".to_string(),
            body_preview: "Hello World".to_string(),
        };

        index.add_message("alice", "INBOX", message.clone()).await.unwrap();

        let mailbox = index.get_mailbox("alice", "INBOX").await.unwrap().unwrap();
        assert_eq!(mailbox.message_count, 1);

        index.delete_message(message.id).await.unwrap();

        let mailbox = index.get_mailbox("alice", "INBOX").await.unwrap().unwrap();
        assert_eq!(mailbox.message_count, 0);
    }

    #[async_std::test]
    async fn test_rename_mailbox() {
        let index = InMemoryIndex::new();

        index.create_mailbox("alice", "Drafts").await.unwrap();

        let uid = index.get_next_uid("alice", "Drafts").await.unwrap();
        let message = IndexedMessage {
            id: MessageId::new(),
            uid,
            mailbox: "Drafts".to_string(),
            flags: vec![],
            internal_date: Utc::now(),
            size: 100,
            from: "alice@example.com".to_string(),
            to: "bob@example.com".to_string(),
            subject: "Draft".to_string(),
            body_preview: "Draft content".to_string(),
        };

        index.add_message("alice", "Drafts", message.clone()).await.unwrap();

        index.rename_mailbox("alice", "Drafts", "MyDrafts").await.unwrap();

        let mailbox = index.get_mailbox("alice", "MyDrafts").await.unwrap();
        assert!(mailbox.is_some());

        let old_mailbox = index.get_mailbox("alice", "Drafts").await.unwrap();
        assert!(old_mailbox.is_none());

        let path = index.get_message_path(message.id).await.unwrap().unwrap();
        assert!(path.contains("MyDrafts"));
    }
}
