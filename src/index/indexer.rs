//! Public Indexer implementation with EventBus integration and write ordering
//!
//! The Indexer wraps an IndexBackend implementation and provides:
//! - Channel-based write ordering for consistency
//! - EventBus integration for reactive updates from FilesystemWatcher
//! - Unique ID mapping for filesystem-to-index coordination

use async_std::channel::{bounded, Receiver, Sender};
use async_std::sync::{Arc, RwLock};
use async_std::task;
use futures::channel::oneshot;
use log::{debug, error, info};
use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::events::{Event, EventBus, EventKind};
use crate::index::backend::IndexBackend;
use crate::index::IndexedMessage;
use crate::types::*;

/// Write commands for the indexer (only writes go through the channel)
enum WriteCommand {
    InitializeMailbox(String, String, oneshot::Sender<Result<()>>),
    RemoveMailboxData(String, String, oneshot::Sender<Result<()>>),
    AddMessage(String, String, IndexedMessage, oneshot::Sender<Result<String>>),
    UpdateFlags(MessageId, Vec<MessageFlag>, oneshot::Sender<Result<()>>),
    DeleteMessage(MessageId, oneshot::Sender<Result<()>>),
    GetNextUid(String, String, oneshot::Sender<Result<Uid>>),
    Shutdown(oneshot::Sender<()>),
}

/// Public indexer that coordinates backend storage with EventBus and write ordering
///
/// This is the main index type used throughout the application. It wraps an
/// IndexBackend implementation and adds:
/// - Write ordering via channels (ensures consistency)
/// - EventBus integration (subscribes to filesystem events)
/// - Unique ID tracking (maps filesystem paths to MessageIds)
pub struct Indexer {
    backend: Arc<RwLock<Box<dyn IndexBackend>>>,
    write_tx: Sender<WriteCommand>,
    unique_id_mapping: Arc<RwLock<HashMap<String, MessageId>>>,
}

impl Indexer {
    /// Create a new Indexer without EventBus integration
    ///
    /// Note: This is internal. Users should use backend-specific constructors
    /// like `create_inmemory_index()` instead.
    pub(crate) fn new(backend: Box<dyn IndexBackend>) -> Self {
        Self::with_event_bus(backend, None)
    }

    /// Create a new Indexer with optional EventBus integration
    ///
    /// If an EventBus is provided, the Indexer will automatically:
    /// - Subscribe to MessageCreated/Modified/Deleted events
    /// - Update the index when filesystem changes occur
    /// - Track unique_id to MessageId mappings
    ///
    /// Note: This is internal. Users should use backend-specific constructors
    /// like `create_inmemory_index_with_eventbus()` instead.
    pub(crate) fn with_event_bus(
        backend: Box<dyn IndexBackend>,
        event_bus: Option<Arc<EventBus>>,
    ) -> Self {
        let backend = Arc::new(RwLock::new(backend));
        let unique_id_mapping = Arc::new(RwLock::new(HashMap::new()));
        let (write_tx, write_rx) = bounded(100);

        // Spawn writer loop
        let backend_clone = backend.clone();
        task::spawn(writer_loop(write_rx, backend_clone));

        // Spawn event listener if EventBus provided
        if let Some(bus) = event_bus {
            let backend_clone = backend.clone();
            let mapping_clone = unique_id_mapping.clone();
            let write_tx_clone = write_tx.clone();
            task::spawn(async move {
                if let Err(e) =
                    event_listener(bus, backend_clone, mapping_clone, write_tx_clone).await
                {
                    error!("Event listener error: {}", e);
                }
            });
        }

        Self {
            backend,
            write_tx,
            unique_id_mapping,
        }
    }

    // Mailbox operations
    // Note: These might eventually move to a separate Mailboxes component
    // For now, they're here for backward compatibility

    /// Initialize a mailbox in the index (creates UID counter, etc.)
    pub async fn initialize_mailbox(&self, username: &str, mailbox: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::InitializeMailbox(
                username.to_string(),
                mailbox.to_string(),
                tx,
            ))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    /// Remove all indexed data for a mailbox
    pub async fn remove_mailbox_data(&self, username: &str, mailbox: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::RemoveMailboxData(
                username.to_string(),
                mailbox.to_string(),
                tx,
            ))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    /// Check if a mailbox exists in the index
    pub async fn mailbox_exists(&self, username: &str, mailbox: &str) -> Result<bool> {
        let backend = self.backend.read().await;
        backend.mailbox_exists(username, mailbox).await
    }

    // Message operations

    /// Add a message to the index
    pub async fn add_message(
        &self,
        username: &str,
        mailbox: &str,
        message: IndexedMessage,
    ) -> Result<String> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::AddMessage(
                username.to_string(),
                mailbox.to_string(),
                message,
                tx,
            ))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    /// Get a message path by its ID
    pub async fn get_message_path(&self, id: MessageId) -> Result<Option<String>> {
        let backend = self.backend.read().await;
        backend.get_message_path(id).await
    }

    /// Get a message path by username, mailbox, and UID
    pub async fn get_message_path_by_uid(
        &self,
        username: &str,
        mailbox: &str,
        uid: Uid,
    ) -> Result<Option<String>> {
        let backend = self.backend.read().await;
        backend.get_message_path_by_uid(username, mailbox, uid).await
    }

    /// List all message paths in a mailbox
    pub async fn list_message_paths(&self, username: &str, mailbox: &str) -> Result<Vec<String>> {
        let backend = self.backend.read().await;
        backend.list_message_paths(username, mailbox).await
    }

    /// Get message metadata by ID
    pub async fn get_message_metadata(&self, id: MessageId) -> Result<Option<IndexedMessage>> {
        let backend = self.backend.read().await;
        backend.get_message_metadata(id).await
    }

    /// Update message flags
    pub async fn update_flags(&self, id: MessageId, flags: Vec<MessageFlag>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::UpdateFlags(id, flags, tx))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    /// Delete a message from the index
    pub async fn delete_message(&self, id: MessageId) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::DeleteMessage(id, tx))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    /// Search for messages matching a query
    pub async fn search(
        &self,
        username: &str,
        mailbox: &str,
        query: &SearchQuery,
    ) -> Result<Vec<String>> {
        let backend = self.backend.read().await;
        backend.search(username, mailbox, query).await
    }

    /// Get the next available UID for a mailbox
    pub async fn get_next_uid(&self, username: &str, mailbox: &str) -> Result<Uid> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::GetNextUid(
                username.to_string(),
                mailbox.to_string(),
                tx,
            ))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    /// Shutdown the indexer gracefully
    pub async fn shutdown(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let _ = self.write_tx.send(WriteCommand::Shutdown(tx)).await;
        let _ = rx.await;
        Ok(())
    }
}

/// Writer loop that processes write commands serially
async fn writer_loop(
    rx: Receiver<WriteCommand>,
    backend: Arc<RwLock<Box<dyn IndexBackend>>>,
) {
    while let Ok(cmd) = rx.recv().await {
        match cmd {
            WriteCommand::InitializeMailbox(username, mailbox, reply) => {
                let mut backend = backend.write().await;
                let result = backend.initialize_mailbox(&username, &mailbox).await;
                let _ = reply.send(result);
            }
            WriteCommand::RemoveMailboxData(username, mailbox, reply) => {
                let mut backend = backend.write().await;
                let result = backend.remove_mailbox_data(&username, &mailbox).await;
                let _ = reply.send(result);
            }
            WriteCommand::AddMessage(username, mailbox, message, reply) => {
                let mut backend = backend.write().await;
                let result = backend.add_message(&username, &mailbox, message).await;
                let _ = reply.send(result);
            }
            WriteCommand::UpdateFlags(id, flags, reply) => {
                let mut backend = backend.write().await;
                let result = backend.update_flags(id, flags).await;
                let _ = reply.send(result);
            }
            WriteCommand::DeleteMessage(id, reply) => {
                let mut backend = backend.write().await;
                let result = backend.delete_message(id).await;
                let _ = reply.send(result);
            }
            WriteCommand::GetNextUid(username, mailbox, reply) => {
                let mut backend = backend.write().await;
                let result = backend.get_next_uid(&username, &mailbox).await;
                let _ = reply.send(result);
            }
            WriteCommand::Shutdown(reply) => {
                let _ = reply.send(());
                break;
            }
        }
    }
}

/// Event listener that subscribes to message events and updates the index
async fn event_listener(
    event_bus: Arc<EventBus>,
    _backend: Arc<RwLock<Box<dyn IndexBackend>>>,
    unique_id_mapping: Arc<RwLock<HashMap<String, MessageId>>>,
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

    info!(
        "Indexer subscribed to message events (subscription: {:?})",
        subscription_id
    );

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
                debug!(
                    "Indexer received MessageCreated event: {}/{}/{}",
                    username, mailbox, unique_id
                );

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
                        error!(
                            "Failed to get next UID for message {}/{}/{}: {}",
                            username, mailbox, unique_id, e
                        );
                        continue;
                    }
                    Err(_) => {
                        error!(
                            "Writer loop dropped UID reply for {}/{}/{}",
                            username, mailbox, unique_id
                        );
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
                        let mut mapping = unique_id_mapping.write().await;
                        mapping.insert(path, message_id);
                        info!("Indexer added message: {}/{}/{}", username, mailbox, unique_id);
                    }
                    Ok(Err(e)) => {
                        error!(
                            "Failed to add message {}/{}/{}: {}",
                            username, mailbox, unique_id, e
                        );
                    }
                    Err(_) => {
                        error!(
                            "Writer loop dropped add_message reply for {}/{}/{}",
                            username, mailbox, unique_id
                        );
                    }
                }
            }
            Event::MessageModified {
                username,
                mailbox,
                unique_id,
                flags,
            } => {
                debug!(
                    "Indexer received MessageModified event: {}/{}/{}",
                    username, mailbox, unique_id
                );

                // Look up the MessageId from unique_id
                let unique_id_key = format!("{}/{}/{}", username, mailbox, unique_id);
                let mapping = unique_id_mapping.read().await;
                let message_id = match mapping.get(&unique_id_key) {
                    Some(&id) => id,
                    None => {
                        error!("Message not found for unique_id: {}", unique_id_key);
                        continue;
                    }
                };
                drop(mapping);

                // Update flags
                let (tx, rx) = oneshot::channel();
                let _ = write_tx
                    .send(WriteCommand::UpdateFlags(message_id, flags, tx))
                    .await;

                match rx.await {
                    Ok(Ok(())) => {
                        info!(
                            "Indexer updated flags for message: {}/{}/{}",
                            username, mailbox, unique_id
                        );
                    }
                    Ok(Err(e)) => {
                        error!(
                            "Failed to update flags for {}/{}/{}: {}",
                            username, mailbox, unique_id, e
                        );
                    }
                    Err(_) => {
                        error!(
                            "Writer loop dropped update_flags reply for {}/{}/{}",
                            username, mailbox, unique_id
                        );
                    }
                }
            }
            Event::MessageDeleted {
                username,
                mailbox,
                unique_id,
            } => {
                debug!(
                    "Indexer received MessageDeleted event: {}/{}/{}",
                    username, mailbox, unique_id
                );

                // Look up and remove the MessageId from unique_id mapping
                let unique_id_key = format!("{}/{}/{}", username, mailbox, unique_id);
                let mut mapping = unique_id_mapping.write().await;
                let message_id = match mapping.remove(&unique_id_key) {
                    Some(id) => id,
                    None => {
                        error!("Message not found for unique_id: {}", unique_id_key);
                        continue;
                    }
                };
                drop(mapping);

                // Delete from index
                let (tx, rx) = oneshot::channel();
                let _ = write_tx
                    .send(WriteCommand::DeleteMessage(message_id, tx))
                    .await;

                match rx.await {
                    Ok(Ok(())) => {
                        info!(
                            "Indexer deleted message: {}/{}/{}",
                            username, mailbox, unique_id
                        );
                    }
                    Ok(Err(e)) => {
                        error!(
                            "Failed to delete message {}/{}/{}: {}",
                            username, mailbox, unique_id, e
                        );
                    }
                    Err(_) => {
                        error!(
                            "Writer loop dropped delete_message reply for {}/{}/{}",
                            username, mailbox, unique_id
                        );
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
