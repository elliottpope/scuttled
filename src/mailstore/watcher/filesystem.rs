//! Filesystem-based mail store watcher implementation
//!
//! Watches mailbox directories for changes and propagates them to the Index
//! and EventBus using a pluggable MailboxFormat parser.

use async_std::channel::{bounded, Receiver, Sender};
use async_std::fs;
use async_std::path::Path as AsyncPath;
use async_std::stream::StreamExt;
use async_std::sync::RwLock;
use async_std::task;
use async_trait::async_trait;
use futures::channel::oneshot;
use log::{debug, error, info, warn};
use notify::{RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::error::{Error, Result};
use crate::events::{Event, EventBus};
use crate::index::{Index, IndexedMessage};
use crate::mailstore::format::MailboxFormat;
use crate::mailstore::MailStoreWatcher;
use crate::types::{MessageFlag, MessageId};

/// Commands for the watcher loop
enum WatchCommand {
    Watch {
        username: String,
        mailbox: String,
        reply: oneshot::Sender<Result<()>>,
    },
    Unwatch {
        username: String,
        mailbox: String,
        reply: oneshot::Sender<Result<()>>,
    },
    Shutdown(oneshot::Sender<()>),
}

/// Internal state for filesystem events
struct WatcherState {
    /// Map from (username, mailbox) to filesystem paths being watched
    watched_mailboxes: HashMap<(String, String), PathBuf>,
    /// Map from filesystem path to (username, mailbox)
    path_to_mailbox: HashMap<PathBuf, (String, String)>,
    /// Map from unique_id to MessageId for tracking messages
    message_mapping: HashMap<String, MessageId>,
}

/// Filesystem-based watcher for mailbox directories
pub struct FilesystemWatcher {
    root: PathBuf,
    index: Arc<dyn Index>,
    event_bus: Arc<EventBus>,
    format: Arc<dyn MailboxFormat>,
    command_tx: Sender<WatchCommand>,
}

impl FilesystemWatcher {
    /// Create a new FilesystemWatcher
    ///
    /// # Arguments
    /// * `root_path` - Root directory for mail storage
    /// * `index` - Index implementation to update when changes are detected
    /// * `event_bus` - Event bus for publishing events
    /// * `format` - Mailbox format parser (e.g., Maildir, mbox)
    pub async fn new<P: AsRef<Path>>(
        root_path: P,
        index: Arc<dyn Index>,
        event_bus: Arc<EventBus>,
        format: Arc<dyn MailboxFormat>,
    ) -> Result<Self> {
        let root_path = root_path.as_ref().to_path_buf();

        // Create root directory if it doesn't exist
        let async_root = AsyncPath::new(&root_path);
        if !async_root.exists().await {
            fs::create_dir_all(&async_root).await?;
        }

        let (command_tx, command_rx) = bounded(100);

        // Spawn the watcher loop
        let root_clone = root_path.clone();
        let index_clone = index.clone();
        let event_bus_clone = event_bus.clone();
        let format_clone = format.clone();
        task::spawn(watcher_loop(
            command_rx,
            root_clone,
            index_clone,
            event_bus_clone,
            format_clone,
        ));

        info!("FilesystemWatcher initialized at {:?}", root_path);

        Ok(Self {
            root: root_path,
            index,
            event_bus,
            format,
            command_tx,
        })
    }
}

#[async_trait]
impl MailStoreWatcher for FilesystemWatcher {
    async fn watch_mailbox(&self, username: &str, mailbox: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(WatchCommand::Watch {
                username: username.to_string(),
                mailbox: mailbox.to_string(),
                reply: tx,
            })
            .await
            .map_err(|_| Error::Internal("Watcher loop stopped".to_string()))?;

        rx.await
            .map_err(|_| Error::Internal("Watcher loop dropped reply".to_string()))?
    }

    async fn unwatch_mailbox(&self, username: &str, mailbox: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(WatchCommand::Unwatch {
                username: username.to_string(),
                mailbox: mailbox.to_string(),
                reply: tx,
            })
            .await
            .map_err(|_| Error::Internal("Watcher loop stopped".to_string()))?;

        rx.await
            .map_err(|_| Error::Internal("Watcher loop dropped reply".to_string()))?
    }

    async fn shutdown(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let _ = self.command_tx.send(WatchCommand::Shutdown(tx)).await;
        let _ = rx.await;
        Ok(())
    }
}

/// Main watcher loop that processes commands and filesystem events
async fn watcher_loop(
    command_rx: Receiver<WatchCommand>,
    root: PathBuf,
    index: Arc<dyn Index>,
    event_bus: Arc<EventBus>,
    format: Arc<dyn MailboxFormat>,
) {
    let state = Arc::new(RwLock::new(WatcherState {
        watched_mailboxes: HashMap::new(),
        path_to_mailbox: HashMap::new(),
        message_mapping: HashMap::new(),
    }));

    // Create the filesystem event channel
    let (event_tx, event_rx) = bounded::<Result<notify::Event>>(1000);

    // Spawn the filesystem event processor
    let state_clone = state.clone();
    let index_clone = index.clone();
    let event_bus_clone = event_bus.clone();
    let root_clone = root.clone();
    let format_clone = format.clone();
    task::spawn(event_processor(
        event_rx,
        state_clone,
        index_clone,
        event_bus_clone,
        root_clone,
        format_clone,
    ));

    // Create the notify watcher
    let event_tx_clone = event_tx.clone();
    let watcher_result = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let event_tx = event_tx_clone.clone();
        task::block_on(async {
            let result = res.map_err(|e| Error::Internal(format!("Watch error: {}", e)));
            let _ = event_tx.send(result).await;
        });
    });

    let mut watcher = match watcher_result {
        Ok(w) => Mutex::new(w),
        Err(e) => {
            error!("Failed to create filesystem watcher: {}", e);
            return;
        }
    };

    // Process commands
    while let Ok(cmd) = command_rx.recv().await {
        match cmd {
            WatchCommand::Watch {
                username,
                mailbox,
                reply,
            } => {
                let result = handle_watch(
                    &mut watcher,
                    &state,
                    &root,
                    &index,
                    &event_bus,
                    &format,
                    &username,
                    &mailbox,
                )
                .await;
                let _ = reply.send(result);
            }
            WatchCommand::Unwatch {
                username,
                mailbox,
                reply,
            } => {
                let result = handle_unwatch(&mut watcher, &state, &username, &mailbox).await;
                let _ = reply.send(result);
            }
            WatchCommand::Shutdown(reply) => {
                info!("Shutting down FilesystemWatcher");
                let _ = reply.send(());
                break;
            }
        }
    }
}

/// Handle the watch command - start watching a mailbox
async fn handle_watch(
    watcher: &Mutex<notify::RecommendedWatcher>,
    state: &Arc<RwLock<WatcherState>>,
    root: &Path,
    index: &Arc<dyn Index>,
    event_bus: &Arc<EventBus>,
    format: &Arc<dyn MailboxFormat>,
    username: &str,
    mailbox: &str,
) -> Result<()> {
    let mailbox_path = root.join(username).join(mailbox);

    // Create mailbox directories based on format requirements
    for subdir in format.watch_subdirectories() {
        let dir_path = mailbox_path.join(subdir);
        let async_dir = AsyncPath::new(&dir_path);
        if !async_dir.exists().await {
            fs::create_dir_all(&async_dir).await?;
        }
    }

    // Add watch for the mailbox directory
    {
        let mut watcher = watcher.lock().unwrap();
        watcher
            .watch(&mailbox_path, RecursiveMode::Recursive)
            .map_err(|e| Error::Internal(format!("Failed to watch directory: {}", e)))?;
    }

    // Update state
    let mut state = state.write().await;
    state.watched_mailboxes.insert(
        (username.to_string(), mailbox.to_string()),
        mailbox_path.clone(),
    );
    state.path_to_mailbox.insert(
        mailbox_path.clone(),
        (username.to_string(), mailbox.to_string()),
    );

    // Scan existing messages and add them to the index
    scan_existing_messages(root, username, mailbox, index, event_bus, format, &mut state).await?;

    info!("Started watching mailbox {}/{}", username, mailbox);
    Ok(())
}

/// Handle the unwatch command - stop watching a mailbox
async fn handle_unwatch(
    watcher: &Mutex<notify::RecommendedWatcher>,
    state: &Arc<RwLock<WatcherState>>,
    username: &str,
    mailbox: &str,
) -> Result<()> {
    let mut state = state.write().await;

    if let Some(path) = state
        .watched_mailboxes
        .remove(&(username.to_string(), mailbox.to_string()))
    {
        {
            let mut watcher = watcher.lock().unwrap();
            watcher
                .unwatch(&path)
                .map_err(|e| Error::Internal(format!("Failed to unwatch directory: {}", e)))?;
        }

        state.path_to_mailbox.remove(&path);
        info!("Stopped watching mailbox {}/{}", username, mailbox);
        Ok(())
    } else {
        Err(Error::NotFound(format!(
            "Mailbox not being watched: {}/{}",
            username, mailbox
        )))
    }
}

/// Scan existing messages in a mailbox and add them to the index
async fn scan_existing_messages(
    root: &Path,
    username: &str,
    mailbox: &str,
    index: &Arc<dyn Index>,
    event_bus: &Arc<EventBus>,
    format: &Arc<dyn MailboxFormat>,
    state: &mut WatcherState,
) -> Result<()> {
    let mailbox_path = root.join(username).join(mailbox);

    for subdir in format.watch_subdirectories() {
        let dir_path = mailbox_path.join(subdir);
        let async_dir = AsyncPath::new(&dir_path);

        if !async_dir.exists().await {
            continue;
        }

        let mut entries = fs::read_dir(&async_dir).await?;
        while let Some(entry) = entries.next().await {
            let entry = entry?;
            let path = entry.path();
            let std_path = path.as_ref();

            if !path.is_file().await {
                continue;
            }

            // Parse the message using the format
            if let Some(parsed) = format.parse_message_path(std_path, root) {
                // Check if this message is already in the index
                let message_path = format!("{}/{}/{}", username, mailbox, parsed.unique_id);
                let existing_paths = index.list_message_paths(username, mailbox).await?;

                if existing_paths.contains(&message_path) {
                    debug!("Message already indexed: {}", message_path);
                    continue;
                }

                // Read the message content
                let content = fs::read(std_path).await?;

                // Parse email headers
                if let Ok(email) = mailparse::parse_mail(&content) {
                    let message_id = MessageId::new();
                    let uid = index.get_next_uid(username, mailbox).await?;

                    // Extract metadata from email
                    let from = email
                        .headers
                        .iter()
                        .find(|h| h.get_key().eq_ignore_ascii_case("from"))
                        .map(|h| h.get_value())
                        .unwrap_or_default();

                    let to = email
                        .headers
                        .iter()
                        .find(|h| h.get_key().eq_ignore_ascii_case("to"))
                        .map(|h| h.get_value())
                        .unwrap_or_default();

                    let subject = email
                        .headers
                        .iter()
                        .find(|h| h.get_key().eq_ignore_ascii_case("subject"))
                        .map(|h| h.get_value())
                        .unwrap_or_default();

                    let body_preview = email
                        .get_body()
                        .unwrap_or_default()
                        .chars()
                        .take(200)
                        .collect();

                    let mut flags = parsed.flags.clone();
                    if parsed.is_new {
                        flags.push(MessageFlag::Recent);
                    }

                    let indexed_message = IndexedMessage {
                        id: message_id,
                        uid,
                        mailbox: mailbox.to_string(),
                        flags,
                        internal_date: chrono::Utc::now(),
                        size: content.len(),
                        from,
                        to,
                        subject,
                        body_preview,
                    };

                    // Add to index
                    index.add_message(username, mailbox, indexed_message).await?;

                    // Track the message
                    state
                        .message_mapping
                        .insert(parsed.unique_id.clone(), message_id);

                    // Publish event
                    let _ = event_bus
                        .publish(Event::MessageCreated {
                            username: username.to_string(),
                            mailbox: mailbox.to_string(),
                            message_id,
                            unique_id: parsed.unique_id.clone(),
                        })
                        .await;

                    debug!("Indexed existing message: {}", message_path);
                }
            }
        }
    }

    Ok(())
}

/// Process filesystem events and update the index accordingly
async fn event_processor(
    event_rx: Receiver<Result<notify::Event>>,
    state: Arc<RwLock<WatcherState>>,
    index: Arc<dyn Index>,
    event_bus: Arc<EventBus>,
    root: PathBuf,
    format: Arc<dyn MailboxFormat>,
) {
    while let Ok(event_result) = event_rx.recv().await {
        match event_result {
            Ok(event) => {
                if let Err(e) =
                    handle_filesystem_event(event, &state, &index, &event_bus, &root, &format).await
                {
                    error!("Error handling filesystem event: {}", e);
                }
            }
            Err(e) => {
                error!("Filesystem watch error: {}", e);
            }
        }
    }
}

/// Handle a single filesystem event
async fn handle_filesystem_event(
    event: notify::Event,
    state: &Arc<RwLock<WatcherState>>,
    index: &Arc<dyn Index>,
    event_bus: &Arc<EventBus>,
    root: &Path,
    format: &Arc<dyn MailboxFormat>,
) -> Result<()> {
    match event.kind {
        notify::EventKind::Create(_) => {
            // New file created
            for path in event.paths {
                handle_create_event(&path, state, index, event_bus, root, format).await?;
            }
        }
        notify::EventKind::Modify(_) => {
            // File modified (renamed when flags change)
            for path in event.paths {
                handle_modify_event(&path, state, index, event_bus, root, format).await?;
            }
        }
        notify::EventKind::Remove(_) => {
            // File deleted
            for path in event.paths {
                handle_remove_event(&path, state, index, event_bus, root, format).await?;
            }
        }
        _ => {
            // Ignore other event types
        }
    }

    Ok(())
}

/// Handle file creation event
async fn handle_create_event(
    path: &Path,
    state: &Arc<RwLock<WatcherState>>,
    index: &Arc<dyn Index>,
    event_bus: &Arc<EventBus>,
    root: &Path,
    format: &Arc<dyn MailboxFormat>,
) -> Result<()> {
    // Parse the path using the format
    if let Some(parsed) = format.parse_message_path(path, root) {
        debug!("New message detected: {:?}", path);

        // Read the message content
        let async_path = AsyncPath::new(path);
        if !async_path.exists().await {
            return Ok(());
        }

        let content = match fs::read(async_path).await {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read new message file: {}", e);
                return Ok(());
            }
        };

        // Parse email headers
        if let Ok(email) = mailparse::parse_mail(&content) {
            let message_id = MessageId::new();
            let uid = index.get_next_uid(&parsed.username, &parsed.mailbox).await?;

            let from = email
                .headers
                .iter()
                .find(|h| h.get_key().eq_ignore_ascii_case("from"))
                .map(|h| h.get_value())
                .unwrap_or_default();

            let to = email
                .headers
                .iter()
                .find(|h| h.get_key().eq_ignore_ascii_case("to"))
                .map(|h| h.get_value())
                .unwrap_or_default();

            let subject = email
                .headers
                .iter()
                .find(|h| h.get_key().eq_ignore_ascii_case("subject"))
                .map(|h| h.get_value())
                .unwrap_or_default();

            let body_preview = email
                .get_body()
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();

            let mut flags = parsed.flags.clone();
            if parsed.is_new {
                flags.push(MessageFlag::Recent);
            }

            let indexed_message = IndexedMessage {
                id: message_id,
                uid,
                mailbox: parsed.mailbox.clone(),
                flags,
                internal_date: chrono::Utc::now(),
                size: content.len(),
                from,
                to,
                subject,
                body_preview,
            };

            // Add to index
            index
                .add_message(&parsed.username, &parsed.mailbox, indexed_message)
                .await?;

            // Track the message
            let mut state = state.write().await;
            state
                .message_mapping
                .insert(parsed.unique_id.clone(), message_id);

            // Publish event
            let _ = event_bus
                .publish(Event::MessageCreated {
                    username: parsed.username.clone(),
                    mailbox: parsed.mailbox.clone(),
                    message_id,
                    unique_id: parsed.unique_id.clone(),
                })
                .await;

            info!(
                "Indexed new message: {}/{}/{}",
                parsed.username, parsed.mailbox, parsed.unique_id
            );
        }
    }

    Ok(())
}

/// Handle file modification event (typically a rename for flag changes)
async fn handle_modify_event(
    path: &Path,
    state: &Arc<RwLock<WatcherState>>,
    index: &Arc<dyn Index>,
    event_bus: &Arc<EventBus>,
    root: &Path,
    format: &Arc<dyn MailboxFormat>,
) -> Result<()> {
    // Parse using format
    if let Some(parsed) = format.parse_message_path(path, root) {
        let state_read = state.read().await;
        if let Some(&message_id) = state_read.message_mapping.get(&parsed.unique_id) {
            drop(state_read);

            debug!("Message flags changed: {:?}", path);

            // Update flags in the index
            let mut flags = parsed.flags.clone();
            if parsed.is_new {
                flags.push(MessageFlag::Recent);
            }

            index.update_flags(message_id, flags.clone()).await?;

            // Publish event
            let _ = event_bus
                .publish(Event::MessageModified {
                    message_id,
                    unique_id: parsed.unique_id.clone(),
                    flags,
                })
                .await;

            info!(
                "Updated message flags: {}/{}/{}",
                parsed.username, parsed.mailbox, parsed.unique_id
            );
        }
    }

    Ok(())
}

/// Handle file removal event
async fn handle_remove_event(
    path: &Path,
    state: &Arc<RwLock<WatcherState>>,
    index: &Arc<dyn Index>,
    event_bus: &Arc<EventBus>,
    root: &Path,
    format: &Arc<dyn MailboxFormat>,
) -> Result<()> {
    if let Some(parsed) = format.parse_message_path(path, root) {
        let mut state_write = state.write().await;
        if let Some(message_id) = state_write.message_mapping.remove(&parsed.unique_id) {
            drop(state_write);

            debug!("Message deleted: {:?}", path);

            // Remove from index
            index.delete_message(message_id).await?;

            // Publish event
            let _ = event_bus
                .publish(Event::MessageDeleted {
                    message_id,
                    unique_id: parsed.unique_id.clone(),
                })
                .await;

            info!(
                "Removed message from index: {}/{}/{}",
                parsed.username, parsed.mailbox, parsed.unique_id
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventBus;
    use crate::index::r#impl::InMemoryIndex;
    use crate::mailstore::watcher::MaildirFormat;
    use tempfile::TempDir;

    #[async_std::test]
    async fn test_watcher_creation() {
        let tmp_dir = TempDir::new().unwrap();
        let index = Arc::new(InMemoryIndex::new());
        let event_bus = Arc::new(EventBus::new());
        let format = Arc::new(MaildirFormat::new());

        let watcher = FilesystemWatcher::new(tmp_dir.path(), index, event_bus, format).await;
        assert!(watcher.is_ok());
    }

    #[async_std::test]
    async fn test_watch_mailbox() {
        let tmp_dir = TempDir::new().unwrap();
        let index = Arc::new(InMemoryIndex::new());
        let event_bus = Arc::new(EventBus::new());
        let format = Arc::new(MaildirFormat::new());

        // Create mailbox in index first
        index.create_mailbox("alice", "INBOX").await.unwrap();

        let watcher = FilesystemWatcher::new(tmp_dir.path(), index, event_bus, format)
            .await
            .unwrap();

        let result = watcher.watch_mailbox("alice", "INBOX").await;
        assert!(result.is_ok());

        // Verify directories were created
        let inbox_path = tmp_dir.path().join("alice/INBOX");
        assert!(inbox_path.join("new").exists());
        assert!(inbox_path.join("cur").exists());
    }
}
