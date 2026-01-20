//! In-memory mailbox registry implementation

use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::RwLock;
use async_trait::async_trait;
use futures::channel::oneshot;
use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::mailboxes::{Mailboxes, MailboxFilter, MailboxInfo};
use crate::types::*;

/// Write commands for the mailbox registry (only writes go through the channel)
enum WriteCommand {
    Create(
        String,
        String,
        oneshot::Sender<Result<MailboxInfo>>,
    ),
    Delete(String, String, oneshot::Sender<Result<()>>),
    GetNextUid(String, String, oneshot::Sender<Result<Uid>>),
    UpdateNextUid(MailboxId, Uid, oneshot::Sender<Result<()>>),
    UpdateUidValidity(MailboxId, u32, oneshot::Sender<Result<()>>),
    UpdateCounts(MailboxId, u32, u32, u32, oneshot::Sender<Result<()>>),
    Shutdown(oneshot::Sender<()>),
}

/// In-memory state for the mailbox registry
struct MailboxState {
    /// Map of (username, mailbox_name) to MailboxInfo
    mailboxes: HashMap<(String, String), MailboxInfo>,
    /// Map of MailboxId to (username, mailbox_name) for reverse lookup
    id_to_key: HashMap<MailboxId, (String, String)>,
    /// Counter for generating unique UID validity values
    next_uid_validity: u32,
}

impl MailboxState {
    fn new() -> Self {
        Self {
            mailboxes: HashMap::new(),
            id_to_key: HashMap::new(),
            next_uid_validity: 1,
        }
    }

    fn create_mailbox(&mut self, username: String, name: String) -> Result<MailboxInfo> {
        let key = (username.clone(), name.clone());

        if self.mailboxes.contains_key(&key) {
            return Err(Error::MailboxExists(name));
        }

        let uid_validity = self.next_uid_validity;
        self.next_uid_validity += 1;

        let mailbox_info = MailboxInfo {
            id: MailboxId {
                username: username.clone(),
                name: name.clone(),
            },
            uid_validity,
            next_uid: 1,
            root_path: format!("{}/{}", username, name),
            message_count: 0,
            unseen_count: 0,
            recent_count: 0,
        };

        self.id_to_key.insert(mailbox_info.id.clone(), key.clone());
        self.mailboxes.insert(key, mailbox_info.clone());

        Ok(mailbox_info)
    }

    fn delete_mailbox(&mut self, username: &str, name: &str) -> Result<()> {
        let key = (username.to_string(), name.to_string());

        if let Some(mailbox_info) = self.mailboxes.remove(&key) {
            self.id_to_key.remove(&mailbox_info.id);
            Ok(())
        } else {
            Err(Error::MailboxNotFound(name.to_string()))
        }
    }

    /// Atomically get the next UID and increment it
    fn get_next_uid(&mut self, username: &str, name: &str) -> Result<Uid> {
        let key = (username.to_string(), name.to_string());

        if let Some(mailbox_info) = self.mailboxes.get_mut(&key) {
            let uid = mailbox_info.next_uid;
            mailbox_info.next_uid += 1;
            Ok(uid)
        } else {
            Err(Error::MailboxNotFound(name.to_string()))
        }
    }

    fn update_next_uid(&mut self, id: &MailboxId, next_uid: Uid) -> Result<()> {
        if let Some(key) = self.id_to_key.get(id) {
            if let Some(mailbox_info) = self.mailboxes.get_mut(key) {
                mailbox_info.next_uid = next_uid;
                return Ok(());
            }
        }
        Err(Error::MailboxNotFound(id.name.clone()))
    }

    fn update_uid_validity(&mut self, id: &MailboxId, uid_validity: u32) -> Result<()> {
        if let Some(key) = self.id_to_key.get(id) {
            if let Some(mailbox_info) = self.mailboxes.get_mut(key) {
                mailbox_info.uid_validity = uid_validity;
                return Ok(());
            }
        }
        Err(Error::MailboxNotFound(id.name.clone()))
    }

    fn update_counts(
        &mut self,
        id: &MailboxId,
        message_count: u32,
        unseen_count: u32,
        recent_count: u32,
    ) -> Result<()> {
        if let Some(key) = self.id_to_key.get(id) {
            if let Some(mailbox_info) = self.mailboxes.get_mut(key) {
                mailbox_info.message_count = message_count;
                mailbox_info.unseen_count = unseen_count;
                mailbox_info.recent_count = recent_count;
                return Ok(());
            }
        }
        Err(Error::MailboxNotFound(id.name.clone()))
    }
}

pub struct InMemoryMailboxes {
    state: Arc<RwLock<MailboxState>>,
    write_tx: Sender<WriteCommand>,
}

impl InMemoryMailboxes {
    pub fn new() -> Self {
        let (write_tx, write_rx) = channel(100);
        let state = Arc::new(RwLock::new(MailboxState::new()));

        let state_clone = Arc::clone(&state);
        tokio::spawn(async move {
            Self::writer_loop(state_clone, write_rx).await;
        });

        Self { state, write_tx }
    }

    async fn writer_loop(state: Arc<RwLock<MailboxState>>, mut rx: Receiver<WriteCommand>) {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                WriteCommand::Create(username, name, reply) => {
                    let mut state = state.write().await;
                    let result = state.create_mailbox(username, name);
                    let _ = reply.send(result);
                }
                WriteCommand::Delete(username, name, reply) => {
                    let mut state = state.write().await;
                    let result = state.delete_mailbox(&username, &name);
                    let _ = reply.send(result);
                }
                WriteCommand::GetNextUid(username, mailbox, reply) => {
                    let mut state = state.write().await;
                    let result = state.get_next_uid(&username, &mailbox);
                    let _ = reply.send(result);
                }
                WriteCommand::UpdateNextUid(id, next_uid, reply) => {
                    let mut state = state.write().await;
                    let result = state.update_next_uid(&id, next_uid);
                    let _ = reply.send(result);
                }
                WriteCommand::UpdateUidValidity(id, uid_validity, reply) => {
                    let mut state = state.write().await;
                    let result = state.update_uid_validity(&id, uid_validity);
                    let _ = reply.send(result);
                }
                WriteCommand::UpdateCounts(id, msg_count, unseen, recent, reply) => {
                    let mut state = state.write().await;
                    let result = state.update_counts(&id, msg_count, unseen, recent);
                    let _ = reply.send(result);
                }
                WriteCommand::Shutdown(reply) => {
                    let _ = reply.send(());
                    break;
                }
            }
        }
    }
}

impl Default for InMemoryMailboxes {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Mailboxes for InMemoryMailboxes {
    async fn create_mailbox(&self, username: &str, name: &str) -> Result<MailboxInfo> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::Create(
                username.to_string(),
                name.to_string(),
                tx,
            ))
            .await
            .map_err(|_| Error::Internal("Failed to send create command".to_string()))?;

        rx.await
            .map_err(|_| Error::Internal("Failed to receive create response".to_string()))?
    }

    async fn get_mailbox(&self, username: &str, name: &str) -> Result<Option<MailboxInfo>> {
        let state = self.state.read().await;
        let key = (username.to_string(), name.to_string());
        Ok(state.mailboxes.get(&key).cloned())
    }

    async fn delete_mailbox(&self, username: &str, name: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::Delete(
                username.to_string(),
                name.to_string(),
                tx,
            ))
            .await
            .map_err(|_| Error::Internal("Failed to send delete command".to_string()))?;

        rx.await
            .map_err(|_| Error::Internal("Failed to receive delete response".to_string()))?
    }

    async fn list_mailboxes(&self, username: &str, filter: &MailboxFilter) -> Result<Vec<MailboxInfo>> {
        let state = self.state.read().await;

        let mailboxes: Vec<MailboxInfo> = state
            .mailboxes
            .iter()
            .filter(|((u, _), _)| u == username)
            .map(|(_, info)| info.clone())
            .collect();

        // Apply the filter
        let filtered = match filter {
            MailboxFilter::All => mailboxes,
            MailboxFilter::Exact(pattern) => {
                mailboxes.into_iter()
                    .filter(|m| {
                        // INBOX is case-insensitive per IMAP RFC 3501
                        if m.id.name.eq_ignore_ascii_case("INBOX") && pattern.eq_ignore_ascii_case("INBOX") {
                            true
                        } else {
                            m.id.name == *pattern
                        }
                    })
                    .collect()
            },
            MailboxFilter::Prefix(prefix) => {
                mailboxes.into_iter()
                    .filter(|m| m.id.name.starts_with(prefix))
                    .collect()
            },
            MailboxFilter::Suffix(suffix) => {
                mailboxes.into_iter()
                    .filter(|m| m.id.name.ends_with(suffix))
                    .collect()
            },
            MailboxFilter::Regex(pattern) => {
                // Use regex crate for pattern matching
                match regex::Regex::new(pattern) {
                    Ok(re) => {
                        mailboxes.into_iter()
                            .filter(|m| re.is_match(&m.id.name))
                            .collect()
                    },
                    Err(_) => {
                        // If regex is invalid, return empty list
                        Vec::new()
                    }
                }
            },
        };

        Ok(filtered)
    }

    async fn get_next_uid(&self, username: &str, mailbox: &str) -> Result<Uid> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::GetNextUid(
                username.to_string(),
                mailbox.to_string(),
                tx,
            ))
            .await
            .map_err(|_| Error::Internal("Failed to send get next UID command".to_string()))?;

        rx.await
            .map_err(|_| Error::Internal("Failed to receive get next UID response".to_string()))?
    }

    async fn update_next_uid(&self, id: &MailboxId, next_uid: Uid) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::UpdateNextUid(id.clone(), next_uid, tx))
            .await
            .map_err(|_| Error::Internal("Failed to send update next UID command".to_string()))?;

        rx.await
            .map_err(|_| Error::Internal("Failed to receive update next UID response".to_string()))?
    }

    async fn update_uid_validity(&self, id: &MailboxId, uid_validity: u32) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::UpdateUidValidity(id.clone(), uid_validity, tx))
            .await
            .map_err(|_| {
                Error::Internal("Failed to send update UID validity command".to_string())
            })?;

        rx.await.map_err(|_| {
            Error::Internal("Failed to receive update UID validity response".to_string())
        })?
    }

    async fn update_counts(
        &self,
        id: &MailboxId,
        message_count: u32,
        unseen_count: u32,
        recent_count: u32,
    ) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::UpdateCounts(
                id.clone(),
                message_count,
                unseen_count,
                recent_count,
                tx,
            ))
            .await
            .map_err(|_| Error::Internal("Failed to send update counts command".to_string()))?;

        rx.await
            .map_err(|_| Error::Internal("Failed to receive update counts response".to_string()))?
    }

    async fn shutdown(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::Shutdown(tx))
            .await
            .map_err(|_| Error::Internal("Failed to send shutdown command".to_string()))?;

        rx.await
            .map_err(|_| Error::Internal("Failed to receive shutdown response".to_string()))?;

        Ok(())
    }
}
