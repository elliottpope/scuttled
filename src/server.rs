//! IMAP server implementation

use async_std::io::{BufReader, WriteExt};
use async_std::net::{TcpListener, TcpStream};
use async_std::prelude::*;
use async_std::sync::Arc;
use std::collections::HashMap;

use crate::error::Result;
use crate::protocol::{parse_command, Command, Response};
use crate::{Authenticator, Index, MailStore, Queue, UserStore, CommandHandler};
use crate::session_context::{SessionContext, SessionState};
use crate::types::*;

/// IMAP server with support for custom command handlers
pub struct ImapServer {
    mail_store: Arc<dyn MailStore>,
    index: Arc<dyn Index>,
    authenticator: Arc<dyn Authenticator>,
    user_store: Arc<dyn UserStore>,
    queue: Arc<dyn Queue>,
    custom_handlers: Arc<HashMap<String, Arc<dyn CommandHandler>>>,
}

impl ImapServer {
    /// Create a new IMAP server with the given stores
    pub fn new<M, I, A, U, Q>(
        mail_store: M,
        index: I,
        authenticator: A,
        user_store: U,
        queue: Q,
    ) -> Self
    where
        M: MailStore + 'static,
        I: Index + 'static,
        A: Authenticator + 'static,
        U: UserStore + 'static,
        Q: Queue + 'static,
    {
        Self {
            mail_store: Arc::new(mail_store),
            index: Arc::new(index),
            authenticator: Arc::new(authenticator),
            user_store: Arc::new(user_store),
            queue: Arc::new(queue),
            custom_handlers: Arc::new(HashMap::new()),
        }
    }

    /// Register a custom command handler
    ///
    /// This allows library users to extend the server with custom IMAP commands.
    pub fn register_handler(&mut self, handler: Arc<dyn CommandHandler>) {
        let handlers = Arc::make_mut(&mut self.custom_handlers);
        handlers.insert(handler.command_name().to_uppercase(), handler);
    }

    /// Start the IMAP server on the specified address
    pub async fn listen(&self, addr: &str) -> Result<()> {
        let listener = TcpListener::bind(addr).await?;
        log::info!("IMAP server listening on {}", addr);
        self.listen_on(listener).await
    }

    /// Listen on an existing TcpListener (useful for testing)
    pub async fn listen_on(&self, listener: TcpListener) -> Result<()> {
        let mut incoming = listener.incoming();
        while let Some(stream) = incoming.next().await {
            let stream = stream?;

            let context = SessionContext {
                mail_store: Arc::clone(&self.mail_store),
                index: Arc::clone(&self.index),
                authenticator: Arc::clone(&self.authenticator),
                user_store: Arc::clone(&self.user_store),
                queue: Arc::clone(&self.queue),
                state: Arc::new(async_std::sync::RwLock::new(SessionState::NotAuthenticated)),
                selected_mailbox: Arc::new(async_std::sync::RwLock::new(None)),
            };

            let session = Session {
                context,
                custom_handlers: Arc::clone(&self.custom_handlers),
            };

            async_std::task::spawn(async move {
                if let Err(e) = session.handle(stream).await {
                    log::error!("Session error: {}", e);
                }
            });
        }

        Ok(())
    }
}

/// Client session
struct Session {
    context: SessionContext,
    custom_handlers: Arc<HashMap<String, Arc<dyn CommandHandler>>>,
}

impl Session {
    async fn handle(&self, stream: TcpStream) -> Result<()> {
        let mut stream = stream;

        stream.write_all(b"* OK IMAP4rev1 Service Ready\r\n").await?;

        let reader = BufReader::new(stream.clone());
        let mut lines = reader.lines();

        while let Some(line) = lines.next().await {
            let line = line?;
            log::debug!("Client: {}", line);

            let response = self.process_line(&line).await;
            let response_str = response.to_string();
            log::debug!("Server: {}", response_str.trim());

            stream.write_all(response_str.as_bytes()).await?;

            if matches!(self.context.get_state().await, SessionState::Logout) {
                break;
            }
        }

        Ok(())
    }

    async fn process_line(&self, line: &str) -> Response {
        let parts: Vec<&str> = line.splitn(2, ' ').collect();
        if parts.len() < 2 {
            return Response::Bad {
                tag: None,
                message: "Invalid command format".to_string(),
            };
        }

        let tag = parts[0];
        let command_line = parts[1];

        match parse_command(tag, command_line) {
            Ok(command) => self.handle_command(tag, command).await,
            Err(e) => Response::Bad {
                tag: Some(tag.to_string()),
                message: format!("Parse error: {}", e),
            },
        }
    }

    async fn handle_command(&self, tag: &str, command: Command) -> Response {
        match command {
            Command::Capability => self.handle_capability(tag).await,
            Command::Noop => Response::Ok {
                tag: Some(tag.to_string()),
                message: "NOOP completed".to_string(),
            },
            Command::Logout => self.handle_logout(tag).await,
            Command::Login { username, password } => self.handle_login(tag, username, password).await,
            Command::Select { mailbox } => self.handle_select(tag, mailbox, false).await,
            Command::Examine { mailbox } => self.handle_select(tag, mailbox, true).await,
            Command::Create { mailbox } => self.handle_create(tag, mailbox).await,
            Command::Delete { mailbox } => self.handle_delete(tag, mailbox).await,
            Command::List { reference, pattern } => self.handle_list(tag, reference, pattern).await,
            Command::Close => self.handle_close(tag).await,
            Command::Expunge => self.handle_expunge(tag).await,
            Command::Custom { name, args } => self.handle_custom(tag, &name, &args).await,
            _ => Response::Bad {
                tag: Some(tag.to_string()),
                message: "Command not implemented".to_string(),
            },
        }
    }

    async fn handle_custom(&self, tag: &str, name: &str, args: &str) -> Response {
        let name_upper = name.to_uppercase();

        if let Some(handler) = self.custom_handlers.get(&name_upper) {
            // Check authentication requirement
            if handler.requires_auth() && !self.context.is_authenticated().await {
                return Response::No {
                    tag: Some(tag.to_string()),
                    message: "Authentication required".to_string(),
                };
            }

            // Check mailbox selection requirement
            if handler.requires_selected_mailbox() && !self.context.has_selected_mailbox().await {
                return Response::No {
                    tag: Some(tag.to_string()),
                    message: "No mailbox selected".to_string(),
                };
            }

            // Clone context for the handler
            let mut handler_context = SessionContext {
                mail_store: Arc::clone(&self.context.mail_store),
                index: Arc::clone(&self.context.index),
                authenticator: Arc::clone(&self.context.authenticator),
                user_store: Arc::clone(&self.context.user_store),
                queue: Arc::clone(&self.context.queue),
                state: Arc::clone(&self.context.state),
                selected_mailbox: Arc::clone(&self.context.selected_mailbox),
            };

            match handler.handle(tag, args, &mut handler_context).await {
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

    async fn handle_capability(&self, tag: &str) -> Response {
        let capabilities = "CAPABILITY IMAP4rev1";
        Response::Untagged {
            message: capabilities.to_string(),
        };
        Response::Ok {
            tag: Some(tag.to_string()),
            message: "CAPABILITY completed".to_string(),
        }
    }

    async fn handle_logout(&self, tag: &str) -> Response {
        self.context.set_state(SessionState::Logout).await;
        Response::Bye {
            message: "IMAP4rev1 Server logging out".to_string(),
        };
        Response::Ok {
            tag: Some(tag.to_string()),
            message: "LOGOUT completed".to_string(),
        }
    }

    async fn handle_login(&self, tag: &str, username: String, password: String) -> Response {
        let creds = Credentials { username: username.clone(), password };

        match self.context.authenticator.authenticate(&creds).await {
            Ok(true) => {
                self.context.set_state(SessionState::Authenticated {
                    username: username.clone(),
                }).await;
                Response::Ok {
                    tag: Some(tag.to_string()),
                    message: format!("{} logged in", username),
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

    async fn handle_select(&self, tag: &str, mailbox: String, readonly: bool) -> Response {
        let username = match self.context.get_username().await {
            Some(u) => u,
            None => return Response::No {
                tag: Some(tag.to_string()),
                message: "Not authenticated".to_string(),
            },
        };

        match self.context.index.get_mailbox(&username, &mailbox).await {
            Ok(Some(_mb)) => {
                self.context.set_state(SessionState::Selected {
                    username,
                    mailbox: mailbox.clone(),
                }).await;
                self.context.set_selected_mailbox(Some(mailbox)).await;

                let cmd = if readonly { "EXAMINE" } else { "SELECT" };
                Response::Ok {
                    tag: Some(tag.to_string()),
                    message: format!("{} completed", cmd),
                }
            }
            Ok(None) => Response::No {
                tag: Some(tag.to_string()),
                message: format!("Mailbox {} not found", mailbox),
            },
            Err(e) => Response::No {
                tag: Some(tag.to_string()),
                message: format!("Error: {}", e),
            },
        }
    }

    async fn handle_create(&self, tag: &str, mailbox: String) -> Response {
        let username = match self.context.get_username().await {
            Some(u) => u,
            None => return Response::No {
                tag: Some(tag.to_string()),
                message: "Not authenticated".to_string(),
            },
        };

        match self.context.index.create_mailbox(&username, &mailbox).await {
            Ok(()) => Response::Ok {
                tag: Some(tag.to_string()),
                message: "CREATE completed".to_string(),
            },
            Err(e) => Response::No {
                tag: Some(tag.to_string()),
                message: format!("Error: {}", e),
            },
        }
    }

    async fn handle_delete(&self, tag: &str, mailbox: String) -> Response {
        let username = match self.context.get_username().await {
            Some(u) => u,
            None => return Response::No {
                tag: Some(tag.to_string()),
                message: "Not authenticated".to_string(),
            },
        };

        // Get all message paths for this mailbox
        match self.context.index.list_message_paths(&username, &mailbox).await {
            Ok(paths) => {
                // Delete all messages from MailStore
                for path in paths {
                    let _ = self.context.mail_store.delete(&path).await;
                }

                // Delete mailbox from Index
                match self.context.index.delete_mailbox(&username, &mailbox).await {
                    Ok(()) => Response::Ok {
                        tag: Some(tag.to_string()),
                        message: "DELETE completed".to_string(),
                    },
                    Err(e) => Response::No {
                        tag: Some(tag.to_string()),
                        message: format!("Error: {}", e),
                    },
                }
            }
            Err(e) => Response::No {
                tag: Some(tag.to_string()),
                message: format!("Error: {}", e),
            },
        }
    }

    async fn handle_list(&self, tag: &str, _reference: String, _pattern: String) -> Response {
        let username = match self.context.get_username().await {
            Some(u) => u,
            None => return Response::No {
                tag: Some(tag.to_string()),
                message: "Not authenticated".to_string(),
            },
        };

        match self.context.index.list_mailboxes(&username).await {
            Ok(mailboxes) => {
                for mailbox in mailboxes {
                    Response::Untagged {
                        message: format!("LIST () \"/\" \"{}\"", mailbox.name),
                    };
                }
                Response::Ok {
                    tag: Some(tag.to_string()),
                    message: "LIST completed".to_string(),
                }
            }
            Err(e) => Response::No {
                tag: Some(tag.to_string()),
                message: format!("Error: {}", e),
            },
        }
    }

    async fn handle_close(&self, tag: &str) -> Response {
        self.context.set_selected_mailbox(None).await;

        if let Some(username) = self.context.get_username().await {
            self.context.set_state(SessionState::Authenticated { username }).await;
        }

        Response::Ok {
            tag: Some(tag.to_string()),
            message: "CLOSE completed".to_string(),
        }
    }

    async fn handle_expunge(&self, tag: &str) -> Response {
        Response::Ok {
            tag: Some(tag.to_string()),
            message: "EXPUNGE completed".to_string(),
        }
    }
}
