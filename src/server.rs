//! IMAP server implementation

use async_std::net::TcpListener;
use async_std::prelude::*;
use async_std::sync::Arc;
use std::collections::HashMap;
use std::time::Duration;

use crate::error::Result;
use crate::{Authenticator, Index, MailStore, Queue, UserStore, CommandHandler, Mailboxes};
use crate::session::Session;

const DEFAULT_MAX_SESSION_DURATION: Duration = Duration::from_secs(3600); // 1 hour
const DEFAULT_MAX_IDLE_DURATION: Duration = Duration::from_secs(1800); // 30 minutes

/// IMAP server with support for custom command handlers
pub struct ImapServer {
    mail_store: Arc<dyn MailStore>,
    index: Arc<dyn Index>,
    authenticator: Arc<dyn Authenticator>,
    user_store: Arc<dyn UserStore>,
    queue: Arc<dyn Queue>,
    mailboxes: Arc<dyn Mailboxes>,
    custom_handlers: Arc<HashMap<String, Arc<dyn CommandHandler>>>,
    max_session_duration: Duration,
    max_idle_duration: Duration,
}

impl ImapServer {
    /// Create a new IMAP server with the given stores
    #[allow(clippy::too_many_arguments)]
    pub fn new<M, I, A, U, Q, MB>(
        mail_store: M,
        index: I,
        authenticator: A,
        user_store: Arc<U>,
        queue: Q,
        mailboxes: MB,
    ) -> Self
    where
        M: MailStore + 'static,
        I: Index + 'static,
        A: Authenticator + 'static,
        U: UserStore + 'static,
        Q: Queue + 'static,
        MB: Mailboxes + 'static,
    {
        Self {
            mail_store: Arc::new(mail_store),
            index: Arc::new(index),
            authenticator: Arc::new(authenticator),
            user_store,
            queue: Arc::new(queue),
            mailboxes: Arc::new(mailboxes),
            custom_handlers: Arc::new(HashMap::new()),
            max_session_duration: DEFAULT_MAX_SESSION_DURATION,
            max_idle_duration: DEFAULT_MAX_IDLE_DURATION,
        }
    }

    /// Set the maximum session duration
    pub fn with_max_session_duration(mut self, duration: Duration) -> Self {
        self.max_session_duration = duration;
        self
    }

    /// Set the maximum idle duration
    pub fn with_max_idle_duration(mut self, duration: Duration) -> Self {
        self.max_idle_duration = duration;
        self
    }

    /// Register a custom command handler
    ///
    /// This allows library users to extend the server with custom IMAP commands.
    pub fn register_handler(&mut self, handler: Arc<dyn CommandHandler>) {
        let handlers = Arc::make_mut(&mut self.custom_handlers);
        handlers.insert(handler.command_name().to_uppercase(), handler);
    }

    /// Create a new session for a client connection
    pub fn new_session(&self) -> Session {
        Session::new(
            Arc::clone(&self.mail_store),
            Arc::clone(&self.index),
            Arc::clone(&self.authenticator),
            Arc::clone(&self.user_store),
            Arc::clone(&self.queue),
            Arc::clone(&self.mailboxes),
            Arc::clone(&self.custom_handlers),
            self.max_session_duration,
            self.max_idle_duration,
        )
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

            let session = self.new_session();

            async_std::task::spawn(async move {
                if let Err(e) = session.handle(stream).await {
                    log::error!("Session error: {}", e);
                }
            });
        }

        Ok(())
    }
}
