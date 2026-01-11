//! Filesystem-based mail store watcher implementation
//!
//! Watches mailbox directories for changes and publishes events to the EventBus
//! using a pluggable MailboxFormat parser. Index and other components subscribe
//! to these events to stay synchronized with filesystem changes.

use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::fs;
use futures::StreamExt;
use tokio::sync::RwLock;
use async_trait::async_trait;
use futures::channel::oneshot;
use log::{debug, error, info, warn};
use notify::{RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::error::{Error, Result};
use crate::events::{Event, EventBus};
use crate::mailstore::format::MailboxFormat;
use crate::mailstore::MailStoreWatcher;
use crate::types::MessageFlag;

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
}

/// Filesystem-based watcher for mailbox directories
pub struct FilesystemWatcher {
    root: PathBuf,
    event_bus: Arc<EventBus>,
    format: Arc<dyn MailboxFormat>,
    command_tx: Sender<WatchCommand>,
}

impl FilesystemWatcher {
    /// Create a new FilesystemWatcher
    ///
    /// # Arguments
    /// * `root_path` - Root directory for mail storage
    /// * `event_bus` - Event bus for publishing events
    /// * `format` - Mailbox format parser (e.g., Maildir, mbox)
    pub async fn new<P: AsRef<Path>>(
        root_path: P,
        event_bus: Arc<EventBus>,
        format: Arc<dyn MailboxFormat>,
    ) -> Result<Self> {
        let root_path = root_path.as_ref().to_path_buf();

        // Create root directory if it doesn't exist
        if !tokio::fs::try_exists(&root_path).await.unwrap_or(false) {
            fs::create_dir_all(&root_path).await?;
        }

        let (command_tx, command_rx) = channel(100);

        // Spawn the watcher loop
        let root_clone = root_path.clone();
        let event_bus_clone = event_bus.clone();
        let format_clone = format.clone();
        tokio::spawn(watcher_loop(
            command_rx,
            root_clone,
            event_bus_clone,
            format_clone,
        ));

        info!("FilesystemWatcher initialized at {:?}", root_path);

        Ok(Self {
            root: root_path,
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
    mut command_rx: Receiver<WatchCommand>,
    root: PathBuf,
    event_bus: Arc<EventBus>,
    format: Arc<dyn MailboxFormat>,
) {
    let state = Arc::new(RwLock::new(WatcherState {
        watched_mailboxes: HashMap::new(),
        path_to_mailbox: HashMap::new(),
    }));

    // Create the filesystem event channel
    let (event_tx, event_rx) = channel::<Result<notify::Event>>(1000);

    // Spawn the filesystem event processor
    let state_clone = state.clone();
    let event_bus_clone = event_bus.clone();
    let root_clone = root.clone();
    let format_clone = format.clone();
    tokio::spawn(event_processor(
        event_rx,
        state_clone,
        event_bus_clone,
        root_clone,
        format_clone,
    ));

    // Create the notify watcher
    let event_tx_clone = event_tx.clone();
    let watcher_result = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let event_tx = event_tx_clone.clone();
        tokio::runtime::Handle::current().block_on(async {
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
    while let Some(cmd) = command_rx.recv().await {
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
    event_bus: &Arc<EventBus>,
    format: &Arc<dyn MailboxFormat>,
    username: &str,
    mailbox: &str,
) -> Result<()> {
    let mailbox_path = root.join(username).join(mailbox);

    // Create mailbox directories based on format requirements
    for subdir in format.watch_subdirectories() {
        let dir_path = mailbox_path.join(subdir);
        if !tokio::fs::try_exists(&dir_path).await.unwrap_or(false) {
            fs::create_dir_all(&dir_path).await?;
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

    // Scan existing messages and publish events for them
    scan_existing_messages(root, username, mailbox, event_bus, format).await?;

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

/// Scan existing messages in a mailbox and publish events for them
async fn scan_existing_messages(
    root: &Path,
    username: &str,
    mailbox: &str,
    event_bus: &Arc<EventBus>,
    format: &Arc<dyn MailboxFormat>,
) -> Result<()> {
    let mailbox_path = root.join(username).join(mailbox);

    for subdir in format.watch_subdirectories() {
        let dir_path = mailbox_path.join(subdir);

        if !tokio::fs::try_exists(&dir_path).await.unwrap_or(false) {
            continue;
        }

        let mut entries = fs::read_dir(&dir_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let std_path = path.as_path();

            let metadata = tokio::fs::metadata(&path).await?;
            if !metadata.is_file() {
                continue;
            }

            // Parse the message using the format
            if let Some(parsed) = format.parse_message_path(std_path, root) {
                // Construct the relative path for the event
                let message_path = format!("{}/{}/{}", username, mailbox, parsed.unique_id);

                // Read the message content
                let content = fs::read(std_path).await?;

                // Parse email headers
                if let Ok(email) = mailparse::parse_mail(&content) {
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

                    let body_preview: String = email
                        .get_body()
                        .unwrap_or_default()
                        .chars()
                        .take(200)
                        .collect();

                    let mut flags = parsed.flags.clone();
                    if parsed.is_new {
                        flags.push(MessageFlag::Recent);
                    }

                    // Publish event - Index will subscribe and handle this
                    let _ = event_bus
                        .publish(Event::MessageCreated {
                            username: username.to_string(),
                            mailbox: mailbox.to_string(),
                            unique_id: parsed.unique_id.clone(),
                            path: message_path.clone(),
                            flags,
                            is_new: parsed.is_new,
                            from,
                            to,
                            subject,
                            body_preview,
                            size: content.len(),
                            internal_date: chrono::Utc::now(),
                        })
                        .await;

                    debug!("Published event for existing message: {}", message_path);
                }
            }
        }
    }

    Ok(())
}

/// Process filesystem events and publish to event bus
async fn event_processor(
    mut event_rx: Receiver<Result<notify::Event>>,
    state: Arc<RwLock<WatcherState>>,
    event_bus: Arc<EventBus>,
    root: PathBuf,
    format: Arc<dyn MailboxFormat>,
) {
    while let Some(event_result) = event_rx.recv().await {
        match event_result {
            Ok(event) => {
                if let Err(e) =
                    handle_filesystem_event(event, &state, &event_bus, &root, &format).await
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
    event_bus: &Arc<EventBus>,
    root: &Path,
    format: &Arc<dyn MailboxFormat>,
) -> Result<()> {
    match event.kind {
        notify::EventKind::Create(_) => {
            // New file created
            for path in event.paths {
                handle_create_event(&path, state, event_bus, root, format).await?;
            }
        }
        notify::EventKind::Modify(_) => {
            // File modified (renamed when flags change)
            for path in event.paths {
                handle_modify_event(&path, state, event_bus, root, format).await?;
            }
        }
        notify::EventKind::Remove(_) => {
            // File deleted
            for path in event.paths {
                handle_remove_event(&path, state, event_bus, root, format).await?;
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
    _state: &Arc<RwLock<WatcherState>>,
    event_bus: &Arc<EventBus>,
    root: &Path,
    format: &Arc<dyn MailboxFormat>,
) -> Result<()> {
    // Parse the path using the format
    if let Some(parsed) = format.parse_message_path(path, root) {
        debug!("New message detected: {:?}", path);

        // Read the message content
        if !tokio::fs::try_exists(path).await.unwrap_or(false) {
            return Ok(());
        }

        let content = match fs::read(path).await {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read new message file: {}", e);
                return Ok(());
            }
        };

        // Parse email headers
        if let Ok(email) = mailparse::parse_mail(&content) {
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

            let body_preview: String = email
                .get_body()
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();

            let mut flags = parsed.flags.clone();
            if parsed.is_new {
                flags.push(MessageFlag::Recent);
            }

            // Construct the relative path for the event
            let message_path = format!("{}/{}/{}", parsed.username, parsed.mailbox, parsed.unique_id);

            // Publish event - Index will subscribe and handle this
            let _ = event_bus
                .publish(Event::MessageCreated {
                    username: parsed.username.clone(),
                    mailbox: parsed.mailbox.clone(),
                    unique_id: parsed.unique_id.clone(),
                    path: message_path,
                    flags,
                    is_new: parsed.is_new,
                    from,
                    to,
                    subject,
                    body_preview,
                    size: content.len(),
                    internal_date: chrono::Utc::now(),
                })
                .await;

            info!(
                "Published event for new message: {}/{}/{}",
                parsed.username, parsed.mailbox, parsed.unique_id
            );
        }
    }

    Ok(())
}

/// Handle file modification event (typically a rename for flag changes)
async fn handle_modify_event(
    path: &Path,
    _state: &Arc<RwLock<WatcherState>>,
    event_bus: &Arc<EventBus>,
    root: &Path,
    format: &Arc<dyn MailboxFormat>,
) -> Result<()> {
    // Parse using format
    if let Some(parsed) = format.parse_message_path(path, root) {
        debug!("Message flags changed: {:?}", path);

        // Build flags from parsed data
        let mut flags = parsed.flags.clone();
        if parsed.is_new {
            flags.push(MessageFlag::Recent);
        }

        // Publish event - Index will subscribe and handle this
        let _ = event_bus
            .publish(Event::MessageModified {
                username: parsed.username.clone(),
                mailbox: parsed.mailbox.clone(),
                unique_id: parsed.unique_id.clone(),
                flags,
            })
            .await;

        info!(
            "Published event for modified message: {}/{}/{}",
            parsed.username, parsed.mailbox, parsed.unique_id
        );
    }

    Ok(())
}

/// Handle file removal event
async fn handle_remove_event(
    path: &Path,
    _state: &Arc<RwLock<WatcherState>>,
    event_bus: &Arc<EventBus>,
    root: &Path,
    format: &Arc<dyn MailboxFormat>,
) -> Result<()> {
    if let Some(parsed) = format.parse_message_path(path, root) {
        debug!("Message deleted: {:?}", path);

        // Publish event - Index will subscribe and handle this
        let _ = event_bus
            .publish(Event::MessageDeleted {
                username: parsed.username.clone(),
                mailbox: parsed.mailbox.clone(),
                unique_id: parsed.unique_id.clone(),
            })
            .await;

        info!(
            "Published event for deleted message: {}/{}/{}",
            parsed.username, parsed.mailbox, parsed.unique_id
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventBus;
    use crate::mailstore::watcher::MaildirFormat;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_watcher_creation() {
        let tmp_dir = TempDir::new().unwrap();
        let event_bus = Arc::new(EventBus::new());
        let format = Arc::new(MaildirFormat::new());

        let watcher = FilesystemWatcher::new(tmp_dir.path(), event_bus, format).await;
        assert!(watcher.is_ok());
    }

    #[tokio::test]
    async fn test_watch_mailbox() {
        let tmp_dir = TempDir::new().unwrap();
        let event_bus = Arc::new(EventBus::new());
        let format = Arc::new(MaildirFormat::new());

        let watcher = FilesystemWatcher::new(tmp_dir.path(), event_bus, format)
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
