//! Session management for IMAP connections
//!
//! This module contains session-related structures for managing IMAP client connections.

use std::collections::{HashSet, HashMap};
use std::time::{Duration, Instant};
use async_std::prelude::*;
use async_std::io::BufReader;
use async_std::sync::{Arc, RwLock};
use async_native_tls::TlsAcceptor;

use crate::error::{Error, Result};
use crate::connection::Connection;
use crate::protocol::{parse_command, Command, Response};
use crate::types::*;
use crate::{Authenticator, Index, MailStore, Queue, UserStore, CommandHandler, Mailboxes};
use crate::session_context::{SessionContext, SessionState};

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
            return Err(Error::ProtocolError("Maximum tag history exceeded".to_string()));
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
    index: Arc<dyn Index>,
    authenticator: Arc<dyn Authenticator>,
    user_store: Arc<dyn UserStore>,
    queue: Arc<dyn Queue>,
    mailboxes: Arc<dyn Mailboxes>,
    custom_handlers: Arc<HashMap<String, Arc<dyn CommandHandler>>>,
}

impl Session {
    /// Create a new session
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mail_store: Arc<dyn MailStore>,
        index: Arc<dyn Index>,
        authenticator: Arc<dyn Authenticator>,
        user_store: Arc<dyn UserStore>,
        queue: Arc<dyn Queue>,
        mailboxes: Arc<dyn Mailboxes>,
        custom_handlers: Arc<HashMap<String, Arc<dyn CommandHandler>>>,
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
            custom_handlers,
        }
    }

    /// Handle the session by processing commands from the connection
    pub async fn handle(mut self, mut connection: Connection, tls_acceptor: Option<Arc<TlsAcceptor>>) -> Result<()> {
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
            let reader = BufReader::new(&connection);
            let mut lines = reader.lines();
            let mut should_continue = false;

            while let Some(line) = lines.next().await {
                self.last_client_interaction = Instant::now();

                // Check session timeouts
                if self.session_started.elapsed() > self.max_session_duration {
                    let _ = self.send_response(&connection, &Response::Bye {
                        message: "Session duration exceeded".to_string(),
                    }).await;
                    return Err(Error::ProtocolError("Session duration exceeded".to_string()));
                }

                if self.last_client_interaction.elapsed() > self.max_idle_duration {
                    let _ = self.send_response(&connection, &Response::Bye {
                        message: "Session idle timeout".to_string(),
                    }).await;
                    return Err(Error::ProtocolError("Session idle timeout".to_string()));
                }

                let line = match line {
                    Ok(l) => l,
                    Err(e) => {
                        eprintln!("Error reading line from {:?}: {}", peer_addr, e);
                        return Ok(());
                    }
                };

                // Parse tag and command
                let parts: Vec<&str> = line.splitn(2, ' ').collect();
                if parts.len() < 2 {
                    let _ = self.send_response(&connection, &Response::Bad {
                        tag: None,
                        message: "Invalid command format".to_string(),
                    }).await;
                    continue;
                }

                let tag = parts[0].to_string();
                let command_line = parts[1];

                // Register tag
                if let Err(e) = self.tag_history.register(tag.clone()) {
                    let _ = self.send_response(&connection, &Response::Bad {
                        tag: Some(tag),
                        message: format!("Tag error: {}", e),
                    }).await;
                    continue;
                }

                // Parse command
                let command = match parse_command(&tag, command_line) {
                    Ok(cmd) => cmd,
                    Err(e) => {
                        let _ = self.send_response(&connection, &Response::Bad {
                            tag: Some(tag),
                            message: format!("Parse error: {}", e),
                        }).await;
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
                        // Drop the lines iterator to release the connection
                        drop(lines);

                        // Upgrade to TLS
                        connection = connection.upgrade_to_tls(tls_acceptor.as_ref().unwrap()).await?;
                        log::info!("Connection upgraded to TLS for {:?}", peer_addr);

                        // Mark that we should continue with the upgraded connection
                        should_continue = true;

                        // Break inner loop to recreate reader with upgraded connection
                        break;
                    }

                    continue;
                }

                // Handle all other commands normally
                let response = self.handle_command(&tag, command, connection.is_tls()).await;
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

    /// Handle a parsed command
    async fn handle_command(&mut self, tag: &str, command: Command, is_tls: bool) -> Response {
        match command {
            Command::Capability => self.handle_capability(tag, is_tls).await,
            Command::Noop => self.handle_noop(tag).await,
            Command::Logout => self.handle_logout(tag).await,
            Command::Starttls => {
                // STARTTLS is handled specially in the main loop, shouldn't reach here
                Response::Bad {
                    tag: Some(tag.to_string()),
                    message: "STARTTLS handling error".to_string(),
                }
            }
            Command::Login { username, password } => {
                self.handle_login(tag, &username, &password).await
            }
            Command::Select { mailbox } => self.handle_select(tag, &mailbox).await,
            Command::Create { mailbox } => self.handle_create(tag, &mailbox).await,
            Command::Delete { mailbox } => self.handle_delete(tag, &mailbox).await,
            Command::List { reference, pattern } => {
                self.handle_list(tag, &reference, &pattern).await
            }
            Command::Custom { name, args } => self.handle_custom(tag, &name, &args).await,
            _ => Response::Bad {
                tag: Some(tag.to_string()),
                message: "Command not yet implemented".to_string(),
            },
        }
    }

    async fn handle_capability(&self, tag: &str, is_tls: bool) -> Response {
        let mut capabilities = vec!["IMAP4rev1"];

        // Only advertise STARTTLS on non-TLS connections
        if !is_tls {
            capabilities.push("STARTTLS");
        }

        Response::Ok {
            tag: Some(tag.to_string()),
            message: format!("CAPABILITY {}", capabilities.join(" ")),
        }
    }

    async fn handle_noop(&self, tag: &str) -> Response {
        Response::Ok {
            tag: Some(tag.to_string()),
            message: "NOOP completed".to_string(),
        }
    }

    async fn handle_logout(&self, tag: &str) -> Response {
        Response::Bye {
            message: format!("{} LOGOUT completed", tag),
        }
    }

    async fn handle_login(&mut self, tag: &str, username: &str, password: &str) -> Response {
        let credentials = Credentials {
            username: username.to_string(),
            password: password.to_string(),
        };

        match self.authenticator.authenticate(&credentials).await {
            Ok(true) => {
                match self.user_store.get_user(username).await {
                    Ok(Some(user)) => {
                        self.authenticated_user = Some(user);
                        Response::Ok {
                            tag: Some(tag.to_string()),
                            message: "LOGIN completed".to_string(),
                        }
                    }
                    Ok(None) => Response::No {
                        tag: Some(tag.to_string()),
                        message: "User not found".to_string(),
                    },
                    Err(e) => Response::No {
                        tag: Some(tag.to_string()),
                        message: format!("Login failed: {}", e),
                    },
                }
            }
            Ok(false) => Response::No {
                tag: Some(tag.to_string()),
                message: "Invalid credentials".to_string(),
            },
            Err(e) => Response::No {
                tag: Some(tag.to_string()),
                message: format!("Authentication error: {}", e),
            },
        }
    }

    async fn handle_select(&mut self, tag: &str, mailbox_name: &str) -> Response {
        let username = match &self.authenticated_user {
            Some(user) => &user.username,
            None => {
                return Response::No {
                    tag: Some(tag.to_string()),
                    message: "Not authenticated".to_string(),
                }
            }
        };

        match self.mailboxes.get_mailbox(username, mailbox_name).await {
            Ok(Some(mailbox)) => {
                self.selected_mailbox = Some(mailbox.id.clone());

                // Initialize sequence map
                match self.index.list_message_paths(username, mailbox_name).await {
                    Ok(_paths) => {
                        // TODO: Get UIDs for these paths
                        self.sequence_map.clear();
                    }
                    Err(_) => {
                        self.sequence_map.clear();
                    }
                }

                Response::Ok {
                    tag: Some(tag.to_string()),
                    message: format!("[UIDVALIDITY {}] SELECT completed, {} messages",
                        mailbox.uid_validity, mailbox.message_count),
                }
            }
            Ok(None) => Response::No {
                tag: Some(tag.to_string()),
                message: "Mailbox not found".to_string(),
            },
            Err(e) => Response::No {
                tag: Some(tag.to_string()),
                message: format!("SELECT failed: {}", e),
            },
        }
    }

    async fn handle_create(&mut self, tag: &str, mailbox_name: &str) -> Response {
        let username = match &self.authenticated_user {
            Some(user) => &user.username,
            None => {
                return Response::No {
                    tag: Some(tag.to_string()),
                    message: "Not authenticated".to_string(),
                }
            }
        };

        match self.mailboxes.create_mailbox(username, mailbox_name).await {
            Ok(_) => Response::Ok {
                tag: Some(tag.to_string()),
                message: "CREATE completed".to_string(),
            },
            Err(e) => Response::No {
                tag: Some(tag.to_string()),
                message: format!("CREATE failed: {}", e),
            },
        }
    }

    async fn handle_delete(&mut self, tag: &str, mailbox_name: &str) -> Response {
        let username = match &self.authenticated_user {
            Some(user) => &user.username,
            None => {
                return Response::No {
                    tag: Some(tag.to_string()),
                    message: "Not authenticated".to_string(),
                }
            }
        };

        match self.mailboxes.delete_mailbox(username, mailbox_name).await {
            Ok(_) => Response::Ok {
                tag: Some(tag.to_string()),
                message: "DELETE completed".to_string(),
            },
            Err(e) => Response::No {
                tag: Some(tag.to_string()),
                message: format!("DELETE failed: {}", e),
            },
        }
    }

    async fn handle_list(&self, tag: &str, _reference: &str, _mailbox_pattern: &str) -> Response {
        let username = match &self.authenticated_user {
            Some(user) => &user.username,
            None => {
                return Response::No {
                    tag: Some(tag.to_string()),
                    message: "Not authenticated".to_string(),
                }
            }
        };

        match self.mailboxes.list_mailboxes(username).await {
            Ok(mailboxes) => {
                // TODO: Format mailbox list response properly
                Response::Ok {
                    tag: Some(tag.to_string()),
                    message: format!("LIST completed, {} mailboxes", mailboxes.len()),
                }
            }
            Err(e) => Response::No {
                tag: Some(tag.to_string()),
                message: format!("LIST failed: {}", e),
            },
        }
    }

    async fn handle_custom(&mut self, tag: &str, name: &str, args: &str) -> Response {
        let name_upper = name.to_uppercase();

        if let Some(handler) = self.custom_handlers.get(&name_upper) {
            // Check authentication requirement
            if handler.requires_auth() && self.authenticated_user.is_none() {
                return Response::No {
                    tag: Some(tag.to_string()),
                    message: "Authentication required".to_string(),
                };
            }

            // Check mailbox selection requirement
            if handler.requires_selected_mailbox() && self.selected_mailbox.is_none() {
                return Response::No {
                    tag: Some(tag.to_string()),
                    message: "No mailbox selected".to_string(),
                };
            }

            // Create context for handler
            let state = if let Some(ref mailbox_id) = self.selected_mailbox {
                if let Some(ref user) = self.authenticated_user {
                    SessionState::Selected {
                        username: user.username.clone(),
                        mailbox: mailbox_id.name.clone(),
                    }
                } else {
                    SessionState::NotAuthenticated
                }
            } else if let Some(ref user) = self.authenticated_user {
                SessionState::Authenticated {
                    username: user.username.clone(),
                }
            } else {
                SessionState::NotAuthenticated
            };

            let mut context = SessionContext {
                mail_store: Arc::clone(&self.mail_store),
                index: Arc::clone(&self.index),
                authenticator: Arc::clone(&self.authenticator),
                user_store: Arc::clone(&self.user_store),
                queue: Arc::clone(&self.queue),
                state: Arc::new(RwLock::new(state)),
                selected_mailbox: Arc::new(RwLock::new(self.selected_mailbox.as_ref().map(|m| m.name.clone()))),
            };

            match handler.handle(tag, args, &mut context).await {
                Ok(response) => response,
                Err(e) => Response::No {
                    tag: Some(tag.to_string()),
                    message: format!("Handler error: {}", e),
                },
            }
        } else {
            Response::Bad {
                tag: Some(tag.to_string()),
                message: format!("Unknown command: {}", name),
            }
        }
    }

    async fn send_response(&self, connection: &Connection, response: &Response) -> Result<()> {
        let response_str = response.to_string();
        (&*connection).write_all(response_str.as_bytes()).await?;
        Ok(())
    }
}
