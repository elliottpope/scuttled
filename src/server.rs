//! IMAP server implementation

use async_std::io::{BufReader, WriteExt};
use async_std::net::{TcpListener, TcpStream};
use async_std::prelude::*;
use async_std::sync::{Arc, RwLock};
use std::collections::HashMap;

use crate::error::Result;
use crate::protocol::{parse_command, Command, Response};
use crate::{Authenticator, Index, MailStore, Queue, UserStore};
use crate::types::*;

/// IMAP server state
pub struct ImapServer<M, I, A, U, Q>
where
    M: MailStore,
    I: Index,
    A: Authenticator,
    U: UserStore,
    Q: Queue,
{
    mail_store: Arc<M>,
    index: Arc<I>,
    authenticator: Arc<A>,
    user_store: Arc<U>,
    queue: Arc<Q>,
}

impl<M, I, A, U, Q> ImapServer<M, I, A, U, Q>
where
    M: MailStore + 'static,
    I: Index + 'static,
    A: Authenticator + 'static,
    U: UserStore + 'static,
    Q: Queue + 'static,
{
    pub fn new(
        mail_store: M,
        index: I,
        authenticator: A,
        user_store: U,
        queue: Q,
    ) -> Self {
        Self {
            mail_store: Arc::new(mail_store),
            index: Arc::new(index),
            authenticator: Arc::new(authenticator),
            user_store: Arc::new(user_store),
            queue: Arc::new(queue),
        }
    }

    /// Start the IMAP server on the specified address
    pub async fn listen(&self, addr: &str) -> Result<()> {
        let listener = TcpListener::bind(addr).await?;
        log::info!("IMAP server listening on {}", addr);

        let mut incoming = listener.incoming();
        while let Some(stream) = incoming.next().await {
            let stream = stream?;
            let session = Session {
                mail_store: Arc::clone(&self.mail_store),
                index: Arc::clone(&self.index),
                authenticator: Arc::clone(&self.authenticator),
                user_store: Arc::clone(&self.user_store),
                queue: Arc::clone(&self.queue),
                state: Arc::new(RwLock::new(SessionState::NotAuthenticated)),
                selected_mailbox: Arc::new(RwLock::new(None)),
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

/// Session state
#[derive(Debug, Clone, PartialEq)]
enum SessionState {
    NotAuthenticated,
    Authenticated { username: Username },
    Selected { username: Username, mailbox: MailboxName },
    Logout,
}

/// Client session
struct Session<M, I, A, U, Q>
where
    M: MailStore,
    I: Index,
    A: Authenticator,
    U: UserStore,
    Q: Queue,
{
    mail_store: Arc<M>,
    index: Arc<I>,
    authenticator: Arc<A>,
    user_store: Arc<U>,
    queue: Arc<Q>,
    state: Arc<RwLock<SessionState>>,
    selected_mailbox: Arc<RwLock<Option<Mailbox>>>,
}

impl<M, I, A, U, Q> Session<M, I, A, U, Q>
where
    M: MailStore,
    I: Index,
    A: Authenticator,
    U: UserStore,
    Q: Queue,
{
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

            let state = self.state.read().await;
            if *state == SessionState::Logout {
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
            Command::Capability => {
                Response::Untagged {
                    message: "CAPABILITY IMAP4rev1 AUTH=PLAIN".to_string(),
                };
                Response::Ok {
                    tag: Some(tag.to_string()),
                    message: "CAPABILITY completed".to_string(),
                }
            }
            Command::Noop => Response::Ok {
                tag: Some(tag.to_string()),
                message: "NOOP completed".to_string(),
            },
            Command::Logout => {
                let mut state = self.state.write().await;
                *state = SessionState::Logout;
                Response::Bye {
                    message: "IMAP4rev1 Server logging out".to_string(),
                }
            }
            Command::Login { username, password } => {
                self.handle_login(tag, username, password).await
            }
            Command::Select { mailbox } => self.handle_select(tag, mailbox, false).await,
            Command::Examine { mailbox } => self.handle_select(tag, mailbox, true).await,
            Command::Create { mailbox } => self.handle_create(tag, mailbox).await,
            Command::Delete { mailbox } => self.handle_delete(tag, mailbox).await,
            Command::List { reference, pattern } => {
                self.handle_list(tag, reference, pattern).await
            }
            Command::Close => self.handle_close(tag).await,
            Command::Expunge => self.handle_expunge(tag).await,
            _ => Response::Bad {
                tag: Some(tag.to_string()),
                message: "Command not implemented".to_string(),
            },
        }
    }

    async fn handle_login(&self, tag: &str, username: String, password: String) -> Response {
        let creds = Credentials { username: username.clone(), password };

        match self.authenticator.authenticate(&creds).await {
            Ok(true) => {
                let mut state = self.state.write().await;
                *state = SessionState::Authenticated { username };
                Response::Ok {
                    tag: Some(tag.to_string()),
                    message: "LOGIN completed".to_string(),
                }
            }
            Ok(false) => Response::No {
                tag: Some(tag.to_string()),
                message: "LOGIN failed".to_string(),
            },
            Err(e) => Response::No {
                tag: Some(tag.to_string()),
                message: format!("LOGIN error: {}", e),
            },
        }
    }

    async fn handle_select(&self, tag: &str, mailbox: String, readonly: bool) -> Response {
        let state = self.state.read().await;
        let username = match &*state {
            SessionState::Authenticated { username } | SessionState::Selected { username, .. } => {
                username.clone()
            }
            _ => {
                return Response::No {
                    tag: Some(tag.to_string()),
                    message: "Must be authenticated".to_string(),
                };
            }
        };
        drop(state);

        match self.mail_store.get_mailbox(&mailbox).await {
            Ok(Some(mb)) => {
                let mut selected = self.selected_mailbox.write().await;
                *selected = Some(mb.clone());

                let mut state = self.state.write().await;
                *state = SessionState::Selected {
                    username,
                    mailbox: mailbox.clone(),
                };

                Response::Ok {
                    tag: Some(tag.to_string()),
                    message: if readonly { "EXAMINE" } else { "SELECT" }.to_string() + " completed",
                }
            }
            Ok(None) => Response::No {
                tag: Some(tag.to_string()),
                message: "Mailbox does not exist".to_string(),
            },
            Err(e) => Response::No {
                tag: Some(tag.to_string()),
                message: format!("SELECT error: {}", e),
            },
        }
    }

    async fn handle_create(&self, tag: &str, mailbox: String) -> Response {
        match self.mail_store.create_mailbox(&mailbox).await {
            Ok(_) => Response::Ok {
                tag: Some(tag.to_string()),
                message: "CREATE completed".to_string(),
            },
            Err(e) => Response::No {
                tag: Some(tag.to_string()),
                message: format!("CREATE error: {}", e),
            },
        }
    }

    async fn handle_delete(&self, tag: &str, mailbox: String) -> Response {
        match self.mail_store.delete_mailbox(&mailbox).await {
            Ok(_) => Response::Ok {
                tag: Some(tag.to_string()),
                message: "DELETE completed".to_string(),
            },
            Err(e) => Response::No {
                tag: Some(tag.to_string()),
                message: format!("DELETE error: {}", e),
            },
        }
    }

    async fn handle_list(&self, tag: &str, _reference: String, pattern: String) -> Response {
        let state = self.state.read().await;
        let username = match &*state {
            SessionState::Authenticated { username } | SessionState::Selected { username, .. } => {
                username.clone()
            }
            _ => {
                return Response::No {
                    tag: Some(tag.to_string()),
                    message: "Must be authenticated".to_string(),
                };
            }
        };
        drop(state);

        match self.mail_store.list_mailboxes(&username).await {
            Ok(mailboxes) => {
                for mb in mailboxes {
                    if pattern == "*" || mb.name.contains(&pattern) {
                        let _untagged = Response::Untagged {
                            message: format!("LIST () \"/\" \"{}\"", mb.name),
                        };
                    }
                }
                Response::Ok {
                    tag: Some(tag.to_string()),
                    message: "LIST completed".to_string(),
                }
            }
            Err(e) => Response::No {
                tag: Some(tag.to_string()),
                message: format!("LIST error: {}", e),
            },
        }
    }

    async fn handle_close(&self, tag: &str) -> Response {
        let mut selected = self.selected_mailbox.write().await;
        *selected = None;

        let mut state = self.state.write().await;
        if let SessionState::Selected { username, .. } = &*state {
            *state = SessionState::Authenticated { username: username.clone() };
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
