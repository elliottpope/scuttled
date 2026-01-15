//! Session management for IMAP connections
//!
//! This module contains session-related structures for managing IMAP client connections.

use tokio_native_tls::TlsAcceptor;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{RwLock, mpsc, broadcast};
use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::index::Indexer;
use crate::protocol::{parse_command, Command, Response};
use crate::session_context::{SessionContext, SessionState};
use crate::types::*;
use crate::{Authenticator, CommandHandlers, MailStore, Mailboxes, Queue, UserStore};

const DEFAULT_MAX_TAG_HISTORY: usize = 10000;

/// Tracks client request tags to prevent duplicates
#[derive(Debug)]
pub struct TagHistory {
    tags: HashSet<String>,
    max_size: usize,
}

impl TagHistory {
    pub fn new(max_size: usize) -> Self {
        Self {
            tags: HashSet::new(),
            max_size,
        }
    }

    pub fn with_default_max() -> Self {
        Self::new(DEFAULT_MAX_TAG_HISTORY)
    }

    /// Register a new tag, returning an error if it already exists or max is exceeded
    pub fn register(&mut self, tag: String) -> Result<()> {
        if self.tags.contains(&tag) {
            return Err(Error::ProtocolError(format!("Duplicate tag: {}", tag)));
        }

        if self.tags.len() >= self.max_size {
            return Err(Error::ProtocolError(
                "Maximum tag history exceeded".to_string(),
            ));
        }

        self.tags.insert(tag);
        Ok(())
    }

    /// Clear all tags (useful for session reset)
    pub fn clear(&mut self) {
        self.tags.clear();
    }
}

/// Lazily initialized map of sequence IDs to UIDs for translating requests
#[derive(Debug, Default)]
pub struct SequenceIdMap {
    map: Option<HashMap<u32, Uid>>,
}

impl SequenceIdMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Initialize the map with a list of UIDs (in sequence order)
    pub fn initialize(&mut self, uids: Vec<Uid>) {
        let mut map = HashMap::new();
        for (seq, uid) in uids.into_iter().enumerate() {
            map.insert((seq + 1) as u32, uid);
        }
        self.map = Some(map);
    }

    /// Get the UID for a sequence number
    pub fn get_uid(&self, sequence_id: u32) -> Result<Uid> {
        self.map
            .as_ref()
            .and_then(|m| m.get(&sequence_id).copied())
            .ok_or_else(|| Error::ProtocolError(format!("Invalid sequence ID: {}", sequence_id)))
    }

    /// Clear the map (when mailbox is deselected)
    pub fn clear(&mut self) {
        self.map = None;
    }

    /// Check if the map is initialized
    pub fn is_initialized(&self) -> bool {
        self.map.is_some()
    }
}

/// IMAP session state and connection handler
pub struct Session {
    // Session timing and limits
    session_started: Instant,
    last_client_interaction: Instant,
    max_session_duration: Duration,
    max_idle_duration: Duration,

    // Tag and sequence tracking
    tag_history: TagHistory,
    sequence_map: SequenceIdMap,

    // Authentication and mailbox state
    authenticated_user: Option<User>,
    selected_mailbox: Option<MailboxId>,

    // Server components
    mail_store: Arc<dyn MailStore>,
    index: Indexer,
    authenticator: Arc<dyn Authenticator>,
    user_store: Arc<dyn UserStore>,
    queue: Arc<dyn Queue>,
    mailboxes: Arc<dyn Mailboxes>,
    command_handlers: CommandHandlers,
}

impl Session {
    /// Create a new session
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mail_store: Arc<dyn MailStore>,
        index: Indexer,
        authenticator: Arc<dyn Authenticator>,
        user_store: Arc<dyn UserStore>,
        queue: Arc<dyn Queue>,
        mailboxes: Arc<dyn Mailboxes>,
        command_handlers: CommandHandlers,
        max_session_duration: Duration,
        max_idle_duration: Duration,
    ) -> Self {
        let now = Instant::now();
        Self {
            session_started: now,
            last_client_interaction: now,
            max_session_duration,
            max_idle_duration,
            tag_history: TagHistory::with_default_max(),
            sequence_map: SequenceIdMap::new(),
            authenticated_user: None,
            selected_mailbox: None,
            mail_store,
            index,
            authenticator,
            user_store,
            queue,
            mailboxes,
            command_handlers,
        }
    }

    /// Handle the session by processing commands from the connection
    pub async fn handle(
        self,
        mut connection: Connection,
        tls_acceptor: Option<Arc<TlsAcceptor>>,
    ) -> Result<()> {
        let peer_addr = connection.peer_addr().ok();

        // Create channels for untagged responses and shutdown signal
        let (_untagged_tx, mut untagged_rx) = mpsc::channel::<Response>(100);
        let (_shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

        // Send greeting
        let greeting = Response::Ok {
            tag: None,
            message: "IMAP server ready".to_string(),
        };
        if let Err(e) = Self::send_response_static(&connection, &greeting).await {
            eprintln!("Failed to send greeting: {}", e);
            return Err(e);
        }

        // Initialize session state as a mutable variable
        let mut session_state = SessionState::NotAuthenticated;
        let mut tag_history = self.tag_history;
        let _sequence_map = self.sequence_map;
        let _authenticated_user = self.authenticated_user;
        let _selected_mailbox = self.selected_mailbox;
        let mut last_client_interaction = self.last_client_interaction;
        let session_started = self.session_started;
        let max_session_duration = self.max_session_duration;
        let max_idle_duration = self.max_idle_duration;
        let command_handlers = self.command_handlers;

        // Create session context (without state management)
        let context = SessionContext {
            mail_store: self.mail_store,
            index: self.index,
            authenticator: self.authenticator,
            user_store: self.user_store,
            queue: self.queue,
            mailboxes: self.mailboxes,
            state: Arc::new(RwLock::new(SessionState::NotAuthenticated)),
            selected_mailbox: Arc::new(RwLock::new(None)),
        };

        // Main command processing loop with tokio::select!
        loop {
            // Create a new reader for this phase
            let mut reader = BufReader::new(&connection);

            loop {
                last_client_interaction = Instant::now();

                // Check session timeouts
                if session_started.elapsed() > max_session_duration {
                    let _ = Self::send_response_static(
                        &connection,
                        &Response::Bye {
                            message: "Session duration exceeded".to_string(),
                        },
                    )
                    .await;
                    return Err(Error::ProtocolError(
                        "Session duration exceeded".to_string(),
                    ));
                }

                if last_client_interaction.elapsed() > max_idle_duration {
                    let _ = Self::send_response_static(
                        &connection,
                        &Response::Bye {
                            message: "Session idle timeout".to_string(),
                        },
                    )
                    .await;
                    return Err(Error::ProtocolError("Session idle timeout".to_string()));
                }

                // Use tokio::select! to handle three futures
                let mut line = String::new();

                tokio::select! {
                    // 1. Incoming untagged responses
                    untagged = untagged_rx.recv() => {
                        if let Some(response) = untagged {
                            if let Err(e) = Self::send_response_static(&connection, &response).await {
                                eprintln!("Error sending untagged response: {}", e);
                                return Ok(());
                            }
                        }
                        continue;
                    }

                    // 2. Incoming lines from TcpStream
                    line_result = reader.read_line(&mut line) => {
                        let line: String = match line_result {
                            Ok(0) => return Ok(()), // EOF
                            Ok(_) => {
                                // Remove trailing newline
                                if line.ends_with('\n') {
                                    line.pop();
                                    if line.ends_with('\r') {
                                        line.pop();
                                    }
                                }
                                line
                            }
                            Err(e) => {
                                eprintln!("Error reading line from {:?}: {}", peer_addr, e);
                                return Ok(());
                            }
                        };

                        // Parse tag and command
                        let parts: Vec<&str> = line.splitn(2, ' ').collect();
                        if parts.len() < 2 {
                            let _ = Self::send_response_static(
                                &connection,
                                &Response::Bad {
                                    tag: None,
                                    message: "Invalid command format".to_string(),
                                },
                            )
                            .await;
                            continue;
                        }

                        let tag = parts[0].to_string();
                        let command_line = parts[1];

                        // Register tag
                        if let Err(e) = tag_history.register(tag.clone()) {
                            let _ = Self::send_response_static(
                                &connection,
                                &Response::Bad {
                                    tag: Some(tag),
                                    message: format!("Tag error: {}", e),
                                },
                            )
                            .await;
                            continue;
                        }

                        // Parse command
                        let command = match parse_command(&tag, command_line) {
                            Ok(cmd) => cmd,
                            Err(e) => {
                                let _ = Self::send_response_static(
                                    &connection,
                                    &Response::Bad {
                                        tag: Some(tag),
                                        message: format!("Parse error: {}", e),
                                    },
                                )
                                .await;
                                continue;
                            }
                        };

                        // Handle STARTTLS specially - needs to upgrade connection
                        if matches!(command, Command::Starttls) {
                            let response = if connection.is_tls() {
                                Response::Bad {
                                    tag: Some(tag.clone()),
                                    message: "Already using TLS".to_string(),
                                }
                            } else if tls_acceptor.is_none() {
                                Response::Bad {
                                    tag: Some(tag.clone()),
                                    message: "STARTTLS not available".to_string(),
                                }
                            } else {
                                Response::Ok {
                                    tag: Some(tag.clone()),
                                    message: "Ready for TLS handshake".to_string(),
                                }
                            };

                            if let Err(e) = Self::send_response_static(&connection, &response).await {
                                eprintln!("Error sending STARTTLS response: {}", e);
                                return Ok(());
                            }

                            // If we sent OK, upgrade the connection
                            if matches!(response, Response::Ok { .. }) {
                                // Drop the reader to release the connection
                                drop(reader);

                                // Upgrade to TLS
                                connection = connection
                                    .upgrade_to_tls(tls_acceptor.as_ref().unwrap())
                                    .await?;
                                log::info!("Connection upgraded to TLS for {:?}", peer_addr);

                                // Break inner loop to recreate reader with upgraded connection
                                break;
                            }

                            continue;
                        }

                        // Handle all other commands
                        let (response, state_update) = Self::handle_command_static(
                            &tag,
                            command,
                            &connection,
                            &command_handlers,
                            &context,
                            &session_state,
                        )
                        .await;

                        // Update session state if necessary
                        if let Some(new_state) = state_update {
                            session_state = new_state;
                        }

                        if let Err(e) = Self::send_response_static(&connection, &response).await {
                            eprintln!("Error sending response: {}", e);
                            return Ok(());
                        }

                        // Check for LOGOUT
                        if matches!(response, Response::Bye { .. }) {
                            return Ok(());
                        }
                    }

                    // 3. Shutdown signal
                    _ = shutdown_rx.recv() => {
                        let _ = Self::send_response_static(
                            &connection,
                            &Response::Bye {
                                message: "Server shutdown".to_string(),
                            },
                        )
                        .await;
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Handle a parsed command (static method for use in main loop)
    async fn handle_command_static(
        tag: &str,
        command: Command,
        connection: &Connection,
        command_handlers: &CommandHandlers,
        context: &SessionContext,
        current_state: &SessionState,
    ) -> (Response, Option<SessionState>) {
        // Extract command name and arguments from Command enum
        // Store owned strings to avoid lifetime issues
        let owned_name;
        let (command_name, args): (&str, String) = match command {
            Command::Capability => ("CAPABILITY", String::new()),
            Command::Noop => ("NOOP", String::new()),
            Command::Logout => ("LOGOUT", String::new()),
            Command::Starttls => {
                // STARTTLS is handled specially in the main loop, shouldn't reach here
                return (Response::Bad {
                    tag: Some(tag.to_string()),
                    message: "STARTTLS handling error".to_string(),
                }, None);
            }
            Command::Login { username, password } => {
                ("LOGIN", format!("{} {}", username, password))
            }
            Command::Select { mailbox } => ("SELECT", mailbox),
            Command::Create { mailbox } => ("CREATE", mailbox),
            Command::Delete { mailbox } => ("DELETE", mailbox),
            Command::List { reference, pattern } => ("LIST", format!("{} {}", reference, pattern)),
            Command::Custom { name, args } => {
                owned_name = name;
                (owned_name.as_str(), args)
            }
            _ => {
                return (Response::Bad {
                    tag: Some(tag.to_string()),
                    message: "Command not yet implemented".to_string(),
                }, None);
            }
        };

        // Delegate to command handlers registry
        command_handlers
            .handle(command_name, tag, &args, connection, context, current_state)
            .await
    }

    async fn send_response_static(mut connection: &Connection, response: &Response) -> Result<()> {
        let response_str = response.to_string();
        connection.write_all(response_str.as_bytes()).await?;
        Ok(())
    }
}
