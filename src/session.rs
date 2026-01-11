//! Session management for IMAP connections
//!
//! This module contains session-related structures for managing IMAP client connections.

use tokio_native_tls::TlsAcceptor;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::RwLock;
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
#[derive(Debug)]
pub struct SequenceIdMap {
    map: Option<HashMap<u32, Uid>>,
}

impl SequenceIdMap {
    pub fn new() -> Self {
        Self { map: None }
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
        mut self,
        mut connection: Connection,
        tls_acceptor: Option<Arc<TlsAcceptor>>,
    ) -> Result<()> {
        let peer_addr = connection.peer_addr().ok();

        // Send greeting
        let greeting = Response::Ok {
            tag: None,
            message: "IMAP server ready".to_string(),
        };
        if let Err(e) = self.send_response(&connection, &greeting).await {
            eprintln!("Failed to send greeting: {}", e);
            return Err(e);
        }

        // Main command processing loop
        loop {
            // Create a new reader for this phase (needed to support STARTTLS upgrade)
            let mut reader = BufReader::new(&connection);
            let mut should_continue = false;

            loop {
                self.last_client_interaction = Instant::now();

                // Check session timeouts
                if self.session_started.elapsed() > self.max_session_duration {
                    let _ = self
                        .send_response(
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

                if self.last_client_interaction.elapsed() > self.max_idle_duration {
                    let _ = self
                        .send_response(
                            &connection,
                            &Response::Bye {
                                message: "Session idle timeout".to_string(),
                            },
                        )
                        .await;
                    return Err(Error::ProtocolError("Session idle timeout".to_string()));
                }

                let mut line = String::new();
                let line_result = reader.read_line(&mut line).await;

                let line: String = match line_result {
                    Ok(0) => break, // EOF
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
                    let _ = self
                        .send_response(
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
                if let Err(e) = self.tag_history.register(tag.clone()) {
                    let _ = self
                        .send_response(
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
                        let _ = self
                            .send_response(
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

                    if let Err(e) = self.send_response(&connection, &response).await {
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

                        // Mark that we should continue with the upgraded connection
                        should_continue = true;

                        // Break inner loop to recreate reader with upgraded connection
                        break;
                    }

                    continue;
                }

                // Handle all other commands normally
                let response = self
                    .handle_command(&tag, command, connection.is_tls())
                    .await;
                if let Err(e) = self.send_response(&connection, &response).await {
                    eprintln!("Error sending response: {}", e);
                    return Ok(());
                }

                // Check for LOGOUT
                if matches!(response, Response::Bye { .. }) {
                    return Ok(());
                }
            }

            // If should_continue is true, we upgraded to TLS and need to continue
            // Otherwise, the connection closed naturally
            if !should_continue {
                return Ok(());
            }
        }
    }

    /// Create a SessionContext from current session state
    async fn create_session_context(&self) -> SessionContext {
        // Determine current session state
        let state = if let Some(ref mailbox) = self.selected_mailbox {
            SessionState::Selected {
                username: self
                    .authenticated_user
                    .as_ref()
                    .map(|u| u.username.clone())
                    .unwrap_or_default(),
                mailbox: mailbox.name.clone(),
            }
        } else if let Some(ref user) = self.authenticated_user {
            SessionState::Authenticated {
                username: user.username.clone(),
            }
        } else {
            SessionState::NotAuthenticated
        };

        SessionContext {
            mail_store: Arc::clone(&self.mail_store),
            index: self.index.clone(),
            authenticator: Arc::clone(&self.authenticator),
            user_store: Arc::clone(&self.user_store),
            queue: Arc::clone(&self.queue),
            mailboxes: Arc::clone(&self.mailboxes),
            state: Arc::new(RwLock::new(state)),
            selected_mailbox: Arc::new(RwLock::new(
                self.selected_mailbox.as_ref().map(|m| m.name.clone()),
            )),
        }
    }

    /// Sync session state from SessionContext back to Session
    async fn sync_from_context(&mut self, context: &SessionContext) {
        let state = context.get_state().await;

        match state {
            SessionState::NotAuthenticated => {
                self.authenticated_user = None;
                self.selected_mailbox = None;
            }
            SessionState::Authenticated { ref username } => {
                // Update authenticated user if changed
                if self.authenticated_user.as_ref().map(|u| &u.username) != Some(username) {
                    if let Ok(Some(user)) = self.user_store.get_user(username).await {
                        self.authenticated_user = Some(user);
                    }
                }
                self.selected_mailbox = None;
            }
            SessionState::Selected {
                ref username,
                ref mailbox,
            } => {
                // Update authenticated user if changed
                if self.authenticated_user.as_ref().map(|u| &u.username) != Some(username) {
                    if let Ok(Some(user)) = self.user_store.get_user(username).await {
                        self.authenticated_user = Some(user);
                    }
                }
                // Update selected mailbox
                self.selected_mailbox = Some(MailboxId {
                    username: username.clone(),
                    name: mailbox.clone(),
                });
            }
            SessionState::Logout => {
                self.authenticated_user = None;
                self.selected_mailbox = None;
            }
        }

        // Sync selected mailbox from context
        if let Some(mailbox_name) = context.get_selected_mailbox().await {
            if let Some(ref user) = self.authenticated_user {
                self.selected_mailbox = Some(MailboxId {
                    username: user.username.clone(),
                    name: mailbox_name,
                });
            }
        }
    }

    /// Handle a parsed command
    async fn handle_command(&mut self, tag: &str, command: Command, _is_tls: bool) -> Response {
        // Extract command name and arguments from Command enum
        // Store owned strings to avoid lifetime issues
        let owned_name;
        let (command_name, args): (&str, String) = match command {
            Command::Capability => ("CAPABILITY", String::new()),
            Command::Noop => ("NOOP", String::new()),
            Command::Logout => ("LOGOUT", String::new()),
            Command::Starttls => {
                // STARTTLS is handled specially in the main loop, shouldn't reach here
                return Response::Bad {
                    tag: Some(tag.to_string()),
                    message: "STARTTLS handling error".to_string(),
                };
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
                return Response::Bad {
                    tag: Some(tag.to_string()),
                    message: "Command not yet implemented".to_string(),
                };
            }
        };

        // Create session context
        let mut context = self.create_session_context().await;

        // Delegate to command handlers registry
        let response = self
            .command_handlers
            .handle(command_name, tag, &args, &mut context)
            .await;

        // Sync session state back from context
        self.sync_from_context(&context).await;

        response
    }

    async fn send_response(&self, connection: &Connection, response: &Response) -> Result<()> {
        let response_str = response.to_string();
        (&*connection).write_all(response_str.as_bytes()).await?;
        Ok(())
    }
}
