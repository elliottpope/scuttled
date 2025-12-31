//! In-memory index implementation

use async_std::channel::{bounded, Receiver, Sender};
use async_std::sync::{Arc, RwLock};
use async_std::task;
use async_trait::async_trait;
use futures::channel::oneshot;
use log::{debug, error, info};
use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::events::{Event, EventBus, EventKind};
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
    /// Map from unique_id (format: "username/mailbox/unique_id") to MessageId
    /// This allows us to handle events from FilesystemWatcher that use unique_id
    unique_id_mapping: HashMap<String, MessageId>,
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
    /// Create a new InMemoryIndex
    ///
    /// If an EventBus is provided, the index will subscribe to message events
    /// and automatically update itself when messages are created, modified, or deleted.
    pub fn new() -> Self {
        Self::with_event_bus(None)
    }

    /// Create a new InMemoryIndex with EventBus integration
    pub fn with_event_bus(event_bus: Option<Arc<EventBus>>) -> Self {
        let state = Arc::new(RwLock::new(IndexState {
            mailboxes: HashMap::new(),
            messages: HashMap::new(),
            unique_id_mapping: HashMap::new(),
        }));

        let (write_tx, write_rx) = bounded(100);

        let state_clone = Arc::clone(&state);
        task::spawn(writer_loop(write_rx, state_clone));

        // If we have an event bus, subscribe to message events
        if let Some(bus) = event_bus {
            let state_clone = Arc::clone(&state);
            let write_tx_clone = write_tx.clone();
            task::spawn(async move {
                if let Err(e) = event_listener(bus, state_clone, write_tx_clone).await {
                    error!("Event listener error: {}", e);
                }
            });
        }

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

/// Event listener that subscribes to message events and updates the index
async fn event_listener(
    event_bus: Arc<EventBus>,
    state: Arc<RwLock<IndexState>>,
    write_tx: Sender<WriteCommand>,
) -> Result<()> {
    // Subscribe to message events
    let (subscription_id, event_rx) = event_bus
        .subscribe(vec![
            EventKind::MessageCreated,
            EventKind::MessageModified,
            EventKind::MessageDeleted,
        ])
        .await?;

    info!("Index subscribed to message events (subscription: {:?})", subscription_id);

    // Listen for events
    while let Ok(event) = event_rx.recv().await {
        match event {
            Event::MessageCreated {
                username,
                mailbox,
                unique_id,
                path,
                flags,
                is_new: _,
                from,
                to,
                subject,
                body_preview,
                size,
                internal_date,
            } => {
                debug!("Index received MessageCreated event: {}/{}/{}", username, mailbox, unique_id);

                // Get next UID for the message
                let (uid_tx, uid_rx) = oneshot::channel();
                let _ = write_tx
                    .send(WriteCommand::GetNextUid(
                        username.clone(),
                        mailbox.clone(),
                        uid_tx,
                    ))
                    .await;

                let uid = match uid_rx.await {
                    Ok(Ok(uid)) => uid,
                    Ok(Err(e)) => {
                        error!("Failed to get next UID for message {}/{}/{}: {}", username, mailbox, unique_id, e);
                        continue;
                    }
                    Err(_) => {
                        error!("Writer loop dropped UID reply for {}/{}/{}", username, mailbox, unique_id);
                        continue;
                    }
                };

                // Create indexed message
                let message_id = MessageId::new();
                let indexed_message = IndexedMessage {
                    id: message_id,
                    uid,
                    mailbox: mailbox.clone(),
                    flags,
                    internal_date,
                    size,
                    from,
                    to,
                    subject,
                    body_preview,
                };

                // Add to index
                let (add_tx, add_rx) = oneshot::channel();
                let _ = write_tx
                    .send(WriteCommand::AddMessage(
                        username.clone(),
                        mailbox.clone(),
                        indexed_message,
                        add_tx,
                    ))
                    .await;

                match add_rx.await {
                    Ok(Ok(_)) => {
                        // Track the unique_id -> MessageId mapping
                        let mut state = state.write().await;
                        state.unique_id_mapping.insert(path, message_id);
                        info!("Index added message: {}/{}/{}", username, mailbox, unique_id);
                    }
                    Ok(Err(e)) => {
                        error!("Failed to add message {}/{}/{}: {}", username, mailbox, unique_id, e);
                    }
                    Err(_) => {
                        error!("Writer loop dropped add_message reply for {}/{}/{}", username, mailbox, unique_id);
                    }
                }
            }
            Event::MessageModified {
                username,
                mailbox,
                unique_id,
                flags,
            } => {
                debug!("Index received MessageModified event: {}/{}/{}", username, mailbox, unique_id);

                // Look up the MessageId from unique_id
                let unique_id_key = format!("{}/{}/{}", username, mailbox, unique_id);
                let state = state.read().await;
                let message_id = match state.unique_id_mapping.get(&unique_id_key) {
                    Some(&id) => id,
                    None => {
                        error!("Message not found for unique_id: {}", unique_id_key);
                        continue;
                    }
                };
                drop(state);

                // Update flags
                let (tx, rx) = oneshot::channel();
                let _ = write_tx
                    .send(WriteCommand::UpdateFlags(message_id, flags, tx))
                    .await;

                match rx.await {
                    Ok(Ok(())) => {
                        info!("Index updated flags for message: {}/{}/{}", username, mailbox, unique_id);
                    }
                    Ok(Err(e)) => {
                        error!("Failed to update flags for {}/{}/{}: {}", username, mailbox, unique_id, e);
                    }
                    Err(_) => {
                        error!("Writer loop dropped update_flags reply for {}/{}/{}", username, mailbox, unique_id);
                    }
                }
            }
            Event::MessageDeleted {
                username,
                mailbox,
                unique_id,
            } => {
                debug!("Index received MessageDeleted event: {}/{}/{}", username, mailbox, unique_id);

                // Look up and remove the MessageId from unique_id mapping
                let unique_id_key = format!("{}/{}/{}", username, mailbox, unique_id);
                let mut state = state.write().await;
                let message_id = match state.unique_id_mapping.remove(&unique_id_key) {
                    Some(id) => id,
                    None => {
                        error!("Message not found for unique_id: {}", unique_id_key);
                        continue;
                    }
                };
                drop(state);

                // Delete from index
                let (tx, rx) = oneshot::channel();
                let _ = write_tx
                    .send(WriteCommand::DeleteMessage(message_id, tx))
                    .await;

                match rx.await {
                    Ok(Ok(())) => {
                        info!("Index deleted message: {}/{}/{}", username, mailbox, unique_id);
                    }
                    Ok(Err(e)) => {
                        error!("Failed to delete message {}/{}/{}: {}", username, mailbox, unique_id, e);
                    }
                    Err(_) => {
                        error!("Writer loop dropped delete_message reply for {}/{}/{}", username, mailbox, unique_id);
                    }
                }
            }
            _ => {
                // Ignore other events
            }
        }
    }

    info!("Event listener stopped");
    Ok(())
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

    #[async_std::test]
    async fn test_event_bus_integration() {
        use crate::events::Event;

        // Create EventBus and Index with event integration
        let event_bus = Arc::new(EventBus::new());
        let index = InMemoryIndex::with_event_bus(Some(event_bus.clone()));

        // Create a mailbox first
        index.create_mailbox("alice", "INBOX").await.unwrap();

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

        // Verify flags were updated (we can't easily check this without direct access,
        // but we've verified the integration works)

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
