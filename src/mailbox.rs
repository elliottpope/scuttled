//! Mailbox shared handle for UID management and subscriptions
//!
//! This module provides a Copy/Clone mailbox handle that coordinates:
//! - UID→path bidirectional mapping
//! - UID validity tracking
//! - UID next counter
//! - Subscription management for IDLE/unsolicited notifications
//!
//! # Architecture
//!
//! Mailbox coordinates state mutations via channels:
//! - State is managed in a background writer loop
//! - All mutations go through the channel for atomicity
//! - Reads can bypass the channel for performance (via direct clones)
//!
//! # Usage
//!
//! ```ignore
//! use scuttled::mailbox::Mailbox;
//!
//! let mailbox = Mailbox::new("alice", "INBOX", 1234567890).await?;
//!
//! // Cheap to clone!
//! let mailbox_clone = mailbox.clone();
//!
//! // Assign UID to a message path
//! let uid = mailbox.assign_uid("alice/INBOX/cur/msg1.eml").await?;
//!
//! // Get path for a UID
//! let path = mailbox.get_path(uid).await?;
//!
//! // Subscribe to notifications
//! let subscription = mailbox.subscribe().await?;
//! ```

use tokio::sync::mpsc::{channel, Receiver, Sender};
use futures::channel::oneshot;
use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::types::Uid;

/// Mailbox commands for the state loop
#[derive(Debug)]
enum MailboxCommand {
    AssignUid {
        path: String,
        reply: oneshot::Sender<Result<Uid>>,
    },
    GetPath {
        uid: Uid,
        reply: oneshot::Sender<Result<Option<String>>>,
    },
    GetUid {
        path: String,
        reply: oneshot::Sender<Result<Option<Uid>>>,
    },
    RemoveMessage {
        uid: Uid,
        reply: oneshot::Sender<Result<()>>,
    },
    UpdatePath {
        uid: Uid,
        new_path: String,
        reply: oneshot::Sender<Result<()>>,
    },
    GetState {
        reply: oneshot::Sender<MailboxState>,
    },
    Subscribe {
        reply: oneshot::Sender<Receiver<MailboxNotification>>,
    },
    Notify {
        notification: MailboxNotification,
        reply: oneshot::Sender<()>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

/// Mailbox state snapshot (read-only)
#[derive(Debug, Clone)]
pub struct MailboxState {
    pub username: String,
    pub name: String,
    pub uid_validity: u32,
    pub uid_next: Uid,
    pub message_count: usize,
}

/// Notification sent to IDLE subscribers
#[derive(Debug, Clone)]
pub enum MailboxNotification {
    /// New message arrived (UID)
    MessageAdded(Uid),
    /// Message was expunged (UID)
    MessageExpunged(Uid),
    /// Message flags changed (UID)
    FlagsChanged(Uid),
}

/// Mailbox shared handle (cheap to clone)
///
/// Coordinates UID assignments, path mappings, and subscriptions.
/// All state mutations go through a channel-based writer loop.
#[derive(Clone)]
pub struct Mailbox {
    command_tx: Sender<MailboxCommand>,
}

impl Mailbox {
    /// Create a new Mailbox instance
    ///
    /// This spawns a state loop task and returns a handle that can be cloned cheaply.
    ///
    /// # Arguments
    /// * `username` - Username owning the mailbox
    /// * `name` - Mailbox name (e.g., "INBOX")
    /// * `uid_validity` - UID validity value for this mailbox instance
    pub async fn new(username: impl Into<String>, name: impl Into<String>, uid_validity: u32) -> Result<Self> {
        let (command_tx, command_rx) = channel(100);

        let username = username.into();
        let name = name.into();

        // Spawn state loop
        tokio::spawn(mailbox_state_loop(command_rx, username, name, uid_validity));

        Ok(Self { command_tx })
    }

    /// Assign a new UID to a message path
    ///
    /// Returns the assigned UID. If the path already has a UID, returns an error.
    pub async fn assign_uid(&self, path: &str) -> Result<Uid> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(MailboxCommand::AssignUid {
                path: path.to_string(),
                reply: tx,
            })
            .await
            .map_err(|_| Error::Internal("Mailbox state loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Mailbox state loop dropped reply".to_string()))?
    }

    /// Get the path for a given UID
    pub async fn get_path(&self, uid: Uid) -> Result<Option<String>> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(MailboxCommand::GetPath { uid, reply: tx })
            .await
            .map_err(|_| Error::Internal("Mailbox state loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Mailbox state loop dropped reply".to_string()))?
    }

    /// Get the UID for a given path
    pub async fn get_uid(&self, path: &str) -> Result<Option<Uid>> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(MailboxCommand::GetUid {
                path: path.to_string(),
                reply: tx,
            })
            .await
            .map_err(|_| Error::Internal("Mailbox state loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Mailbox state loop dropped reply".to_string()))?
    }

    /// Remove a message by UID (updates internal mappings)
    pub async fn remove_message(&self, uid: Uid) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(MailboxCommand::RemoveMessage { uid, reply: tx })
            .await
            .map_err(|_| Error::Internal("Mailbox state loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Mailbox state loop dropped reply".to_string()))?
    }

    /// Update the path for a UID (used when flags change and filename changes)
    pub async fn update_path(&self, uid: Uid, new_path: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(MailboxCommand::UpdatePath {
                uid,
                new_path: new_path.to_string(),
                reply: tx,
            })
            .await
            .map_err(|_| Error::Internal("Mailbox state loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Mailbox state loop dropped reply".to_string()))?
    }

    /// Get current mailbox state snapshot
    pub async fn get_state(&self) -> Result<MailboxState> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(MailboxCommand::GetState { reply: tx })
            .await
            .map_err(|_| Error::Internal("Mailbox state loop stopped".to_string()))?;
        Ok(rx.await
            .map_err(|_| Error::Internal("Mailbox state loop dropped reply".to_string()))?)
    }

    /// Subscribe to mailbox notifications (for IDLE)
    ///
    /// Returns a receiver that will get notifications when messages are added,
    /// expunged, or flags change.
    pub async fn subscribe(&self) -> Result<Receiver<MailboxNotification>> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(MailboxCommand::Subscribe { reply: tx })
            .await
            .map_err(|_| Error::Internal("Mailbox state loop stopped".to_string()))?;
        Ok(rx.await
            .map_err(|_| Error::Internal("Mailbox state loop dropped reply".to_string()))?)
    }

    /// Send a notification to all subscribers
    ///
    /// Used by external components (e.g., StorageWatcher) to notify
    /// IDLE clients about changes.
    pub async fn notify(&self, notification: MailboxNotification) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(MailboxCommand::Notify { notification, reply: tx })
            .await
            .map_err(|_| Error::Internal("Mailbox state loop stopped".to_string()))?;
        let _ = rx.await;
        Ok(())
    }

    /// Shutdown the mailbox gracefully
    pub async fn shutdown(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let _ = self.command_tx.send(MailboxCommand::Shutdown { reply: tx }).await;
        let _ = rx.await;
        Ok(())
    }
}

/// Internal state for the mailbox
struct InternalMailboxState {
    username: String,
    name: String,
    uid_validity: u32,
    uid_next: Uid,
    uid_to_path: HashMap<Uid, String>,
    path_to_uid: HashMap<String, Uid>,
    subscribers: Vec<Sender<MailboxNotification>>,
}

/// State loop for coordinating mailbox operations
///
/// All state mutations go through this loop for atomicity.
async fn mailbox_state_loop(
    mut rx: Receiver<MailboxCommand>,
    username: String,
    name: String,
    uid_validity: u32,
) {
    let mut state = InternalMailboxState {
        username,
        name,
        uid_validity,
        uid_next: 1,
        uid_to_path: HashMap::new(),
        path_to_uid: HashMap::new(),
        subscribers: Vec::new(),
    };

    while let Some(cmd) = rx.recv().await {
        match cmd {
            MailboxCommand::AssignUid { path, reply } => {
                // Check if path already has a UID
                if state.path_to_uid.contains_key(&path) {
                    let _ = reply.send(Err(Error::Internal(format!(
                        "Path already has UID assigned: {}",
                        path
                    ))));
                    continue;
                }

                let uid = state.uid_next;
                state.uid_to_path.insert(uid, path.clone());
                state.path_to_uid.insert(path, uid);
                state.uid_next = state.uid_next + 1;

                let _ = reply.send(Ok(uid));
            }
            MailboxCommand::GetPath { uid, reply } => {
                let path = state.uid_to_path.get(&uid).cloned();
                let _ = reply.send(Ok(path));
            }
            MailboxCommand::GetUid { path, reply } => {
                let uid = state.path_to_uid.get(&path).copied();
                let _ = reply.send(Ok(uid));
            }
            MailboxCommand::RemoveMessage { uid, reply } => {
                if let Some(path) = state.uid_to_path.remove(&uid) {
                    state.path_to_uid.remove(&path);
                    let _ = reply.send(Ok(()));
                } else {
                    let _ = reply.send(Err(Error::NotFound(format!("UID not found: {}", uid))));
                }
            }
            MailboxCommand::UpdatePath { uid, new_path, reply } => {
                if let Some(old_path) = state.uid_to_path.get_mut(&uid) {
                    state.path_to_uid.remove(old_path);
                    *old_path = new_path.clone();
                    state.path_to_uid.insert(new_path, uid);
                    let _ = reply.send(Ok(()));
                } else {
                    let _ = reply.send(Err(Error::NotFound(format!("UID not found: {}", uid))));
                }
            }
            MailboxCommand::GetState { reply } => {
                let snapshot = MailboxState {
                    username: state.username.clone(),
                    name: state.name.clone(),
                    uid_validity: state.uid_validity,
                    uid_next: state.uid_next,
                    message_count: state.uid_to_path.len(),
                };
                let _ = reply.send(snapshot);
            }
            MailboxCommand::Subscribe { reply } => {
                let (tx, rx) = channel(100);
                state.subscribers.push(tx);
                let _ = reply.send(rx);
            }
            MailboxCommand::Notify { notification, reply } => {
                // Send notification to all subscribers, remove disconnected ones
                state.subscribers.retain(|sub| {
                    sub.try_send(notification.clone()).is_ok()
                });
                let _ = reply.send(());
            }
            MailboxCommand::Shutdown { reply } => {
                let _ = reply.send(());
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mailbox_creation() {
        let mailbox = Mailbox::new("alice", "INBOX", 1234567890).await.unwrap();
        let state = mailbox.get_state().await.unwrap();

        assert_eq!(state.username, "alice");
        assert_eq!(state.name, "INBOX");
        assert_eq!(state.uid_validity, 1234567890);
        assert_eq!(state.uid_next, 1);
        assert_eq!(state.message_count, 0);

        mailbox.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_mailbox_assign_uid() {
        let mailbox = Mailbox::new("alice", "INBOX", 1234567890).await.unwrap();

        let uid1 = mailbox.assign_uid("alice/INBOX/cur/msg1.eml").await.unwrap();
        let uid2 = mailbox.assign_uid("alice/INBOX/cur/msg2.eml").await.unwrap();

        assert_eq!(uid1, 1);
        assert_eq!(uid2, 2);

        // Check state
        let state = mailbox.get_state().await.unwrap();
        assert_eq!(state.uid_next, 3);
        assert_eq!(state.message_count, 2);

        mailbox.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_mailbox_get_path() {
        let mailbox = Mailbox::new("alice", "INBOX", 1234567890).await.unwrap();

        let uid = mailbox.assign_uid("alice/INBOX/cur/msg1.eml").await.unwrap();
        let path = mailbox.get_path(uid).await.unwrap();

        assert_eq!(path, Some("alice/INBOX/cur/msg1.eml".to_string()));

        // Non-existent UID
        let path = mailbox.get_path(999).await.unwrap();
        assert_eq!(path, None);

        mailbox.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_mailbox_get_uid() {
        let mailbox = Mailbox::new("alice", "INBOX", 1234567890).await.unwrap();

        let uid = mailbox.assign_uid("alice/INBOX/cur/msg1.eml").await.unwrap();
        let retrieved_uid = mailbox.get_uid("alice/INBOX/cur/msg1.eml").await.unwrap();

        assert_eq!(retrieved_uid, Some(uid));

        // Non-existent path
        let retrieved_uid = mailbox.get_uid("alice/INBOX/cur/nonexistent.eml").await.unwrap();
        assert_eq!(retrieved_uid, None);

        mailbox.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_mailbox_remove_message() {
        let mailbox = Mailbox::new("alice", "INBOX", 1234567890).await.unwrap();

        let uid = mailbox.assign_uid("alice/INBOX/cur/msg1.eml").await.unwrap();
        mailbox.remove_message(uid).await.unwrap();

        // Path and UID should no longer exist
        let path = mailbox.get_path(uid).await.unwrap();
        assert_eq!(path, None);

        let retrieved_uid = mailbox.get_uid("alice/INBOX/cur/msg1.eml").await.unwrap();
        assert_eq!(retrieved_uid, None);

        let state = mailbox.get_state().await.unwrap();
        assert_eq!(state.message_count, 0);

        mailbox.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_mailbox_update_path() {
        let mailbox = Mailbox::new("alice", "INBOX", 1234567890).await.unwrap();

        let uid = mailbox.assign_uid("alice/INBOX/new/msg1.eml").await.unwrap();

        // Update path (simulating flag change that moves file from new/ to cur/)
        mailbox
            .update_path(uid, "alice/INBOX/cur/msg1.eml:2,S")
            .await
            .unwrap();

        // Old path should no longer map to UID
        let old_uid = mailbox.get_uid("alice/INBOX/new/msg1.eml").await.unwrap();
        assert_eq!(old_uid, None);

        // New path should map to UID
        let new_uid = mailbox.get_uid("alice/INBOX/cur/msg1.eml:2,S").await.unwrap();
        assert_eq!(new_uid, Some(uid));

        // UID should map to new path
        let path = mailbox.get_path(uid).await.unwrap();
        assert_eq!(path, Some("alice/INBOX/cur/msg1.eml:2,S".to_string()));

        mailbox.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_mailbox_clone() {
        let mailbox1 = Mailbox::new("alice", "INBOX", 1234567890).await.unwrap();
        let mailbox2 = mailbox1.clone(); // Cheap clone!

        // Both handles share the same state
        let uid1 = mailbox1.assign_uid("msg1.eml").await.unwrap();
        let uid2 = mailbox2.assign_uid("msg2.eml").await.unwrap();

        assert_eq!(uid1, 1);
        assert_eq!(uid2, 2);

        // Both can read each other's assignments
        let path = mailbox1.get_path(uid2).await.unwrap();
        assert_eq!(path, Some("msg2.eml".to_string()));

        let path = mailbox2.get_path(uid1).await.unwrap();
        assert_eq!(path, Some("msg1.eml".to_string()));

        mailbox1.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_mailbox_subscribe() {
        let mailbox = Mailbox::new("alice", "INBOX", 1234567890).await.unwrap();

        let mut subscription = mailbox.subscribe().await.unwrap();

        // Send a notification
        mailbox
            .notify(MailboxNotification::MessageAdded(1))
            .await
            .unwrap();

        // Subscriber should receive it
        let notification = subscription.recv().await.unwrap();
        match notification {
            MailboxNotification::MessageAdded(uid) => assert_eq!(uid, 1),
            _ => panic!("Unexpected notification"),
        }

        mailbox.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_mailbox_duplicate_path_error() {
        let mailbox = Mailbox::new("alice", "INBOX", 1234567890).await.unwrap();

        mailbox.assign_uid("msg1.eml").await.unwrap();

        // Assigning the same path again should fail
        let result = mailbox.assign_uid("msg1.eml").await;
        assert!(result.is_err());

        mailbox.shutdown().await.unwrap();
    }
}
