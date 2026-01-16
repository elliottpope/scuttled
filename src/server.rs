//! IMAP server implementation

use tokio_native_tls::TlsAcceptor;
use tokio::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use crate::command_handlers::CommandHandlers;
use crate::connection::Connection;
use crate::error::Result;
use crate::handlers::*;
use crate::index::Indexer;
use crate::session::Session;
use crate::{Authenticator, MailStore, Mailboxes, Queue, UserStore};

const DEFAULT_MAX_SESSION_DURATION: Duration = Duration::from_secs(3600); // 1 hour
const DEFAULT_MAX_IDLE_DURATION: Duration = Duration::from_secs(1800); // 30 minutes

/// IMAP server with support for custom command handlers
#[derive(Clone)]
pub struct ImapServer {
    mail_store: Arc<dyn MailStore>,
    index: Indexer,
    authenticator: Arc<dyn Authenticator>,
    user_store: Arc<dyn UserStore>,
    queue: Arc<dyn Queue>,
    mailboxes: Arc<dyn Mailboxes>,
    command_handlers: Arc<CommandHandlers>,
    max_session_duration: Duration,
    max_idle_duration: Duration,
    tls_acceptor: Option<Arc<TlsAcceptor>>,
}

impl ImapServer {
    /// Create a new IMAP server with the given stores
    #[allow(clippy::too_many_arguments)]
    pub fn new<M, A, U, Q, MB>(
        mail_store: M,
        index: Indexer,
        authenticator: A,
        user_store: Arc<U>,
        queue: Q,
        mailboxes: Arc<MB>,
    ) -> Self
    where
        M: MailStore + 'static,
        A: Authenticator + 'static,
        U: UserStore + 'static,
        Q: Queue + 'static,
        MB: Mailboxes + 'static,
    {
        // Initialize command handlers with built-in handlers
        let mut handlers = CommandHandlers::new();

        // Register all built-in IMAP command handlers
        // Note: CapabilityHandler always advertises STARTTLS - the command
        // will fail gracefully if connection is already TLS
        let _ = handlers.register(Arc::new(CapabilityHandler::new(false)));
        let _ = handlers.register(Arc::new(NoopHandler::new()));
        let _ = handlers.register(Arc::new(LogoutHandler::new()));
        let _ = handlers.register(Arc::new(LoginHandler::new()));
        let _ = handlers.register(Arc::new(SelectHandler::new()));
        let _ = handlers.register(Arc::new(CreateHandler::new()));
        let _ = handlers.register(Arc::new(DeleteHandler::new()));
        let _ = handlers.register(Arc::new(ListHandler::new()));

        Self {
            mail_store: Arc::new(mail_store),
            index,
            authenticator: Arc::new(authenticator),
            user_store,
            queue: Arc::new(queue),
            mailboxes,
            command_handlers: Arc::new(handlers),
            max_session_duration: DEFAULT_MAX_SESSION_DURATION,
            max_idle_duration: DEFAULT_MAX_IDLE_DURATION,
            tls_acceptor: None,
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

    /// Enable TLS with the given identity
    ///
    /// This enables both implicit TLS (via `listen_tls()`) and STARTTLS support
    /// (via `listen()` with STARTTLS command).
    ///
    /// # Arguments
    ///
    /// * `identity` - A native_tls::Identity containing the certificate and private key
    pub fn with_tls_identity(mut self, identity: native_tls::Identity) -> Result<Self> {
        let acceptor = native_tls::TlsAcceptor::new(identity).map_err(|e| {
            crate::error::Error::TlsError(format!("Failed to create TLS acceptor: {}", e))
        })?;
        self.tls_acceptor = Some(Arc::new(TlsAcceptor::from(acceptor)));
        Ok(self)
    }

    /// Enable TLS with certificate and key from PEM files
    ///
    /// # Arguments
    ///
    /// * `cert_pem` - PEM-encoded certificate
    /// * `key_pem` - PEM-encoded private key
    pub fn with_tls_pem(self, cert_pem: &[u8], key_pem: &[u8]) -> Result<Self> {
        let identity = native_tls::Identity::from_pkcs8(cert_pem, key_pem).map_err(|e| {
            crate::error::Error::TlsError(format!("Failed to create identity from PEM: {}", e))
        })?;
        self.with_tls_identity(identity)
    }

    /// Register a custom command handler
    ///
    /// This allows library users to extend the server with custom IMAP commands.
    pub fn register_handler(
        &mut self,
        handler: Arc<dyn crate::command_handler::CommandHandler>,
    ) -> Result<()> {
        let handlers = Arc::make_mut(&mut self.command_handlers);
        handlers.register(handler)
    }

    /// Create a new session for a client connection
    pub fn new_session(&self) -> Session {
        // Clone the Arc<CommandHandlers> to share the registry with the session
        let handlers = (*self.command_handlers).clone();

        Session::new(
            Arc::clone(&self.mail_store),
            self.index.clone(),
            Arc::clone(&self.authenticator),
            Arc::clone(&self.user_store),
            Arc::clone(&self.queue),
            Arc::clone(&self.mailboxes),
            handlers,
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
        loop {
            let (stream, _) = listener.accept().await?;
            let connection = match Connection::plain(stream) {
                Ok(conn) => conn,
                Err(e) => {
                    log::error!("Failed to create connection: {}", e);
                    continue;
                }
            };

            let session = self.new_session();
            let tls_acceptor = self.tls_acceptor.as_ref().map(Arc::clone);

            tokio::spawn(async move {
                if let Err(e) = session.handle(connection, tls_acceptor).await {
                    log::error!("Session error: {}", e);
                }
            });
        }
    }

    /// Start the IMAP server on the specified address with implicit TLS
    ///
    /// This method listens on the given address and immediately wraps all
    /// incoming connections with TLS (typically used on port 993).
    ///
    /// # Errors
    ///
    /// Returns an error if TLS is not configured via `with_tls_identity()` or `with_tls_pem()`
    pub async fn listen_tls(&self, addr: &str) -> Result<()> {
        let acceptor = self.tls_acceptor.as_ref().ok_or_else(|| {
            crate::error::Error::TlsError(
                "TLS not configured. Call with_tls_identity() or with_tls_pem() first.".to_string(),
            )
        })?;

        let listener = TcpListener::bind(addr).await?;
        log::info!("IMAP server listening on {} (implicit TLS)", addr);

        loop {
            let (stream, addr) = listener.accept().await?;

            // Perform TLS handshake immediately
            let tls_stream = acceptor.accept(stream).await.map_err(|e| {
                crate::error::Error::TlsError(format!("TLS handshake failed: {}", e))
            })?;

            let connection = Connection::tls(tls_stream, addr);

            let session = self.new_session();
            let tls_acceptor = Some(Arc::clone(acceptor));

            tokio::spawn(async move {
                if let Err(e) = session.handle(connection, tls_acceptor).await {
                    log::error!("Session error: {}", e);
                }
            });
        }
    }

    /// Listen on an existing TcpListener with implicit TLS (useful for testing)
    ///
    /// This method accepts incoming connections and immediately wraps them with TLS.
    ///
    /// # Errors
    ///
    /// Returns an error if TLS is not configured via `with_tls_identity()` or `with_tls_pem()`
    pub async fn listen_on_tls(&self, listener: TcpListener) -> Result<()> {
        let acceptor = self.tls_acceptor.as_ref().ok_or_else(|| {
            crate::error::Error::TlsError(
                "TLS not configured. Call with_tls_identity() or with_tls_pem() first.".to_string(),
            )
        })?;

        loop {
            let (stream, addr) = listener.accept().await?;

            // Perform TLS handshake immediately
            let tls_stream = acceptor.accept(stream).await.map_err(|e| {
                crate::error::Error::TlsError(format!("TLS handshake failed: {}", e))
            })?;

            let connection = Connection::tls(tls_stream, addr);

            let session = self.new_session();
            let tls_acceptor = Some(Arc::clone(acceptor));

            tokio::spawn(async move {
                if let Err(e) = session.handle(connection, tls_acceptor).await {
                    log::error!("Session error: {}", e);
                }
            });
        }
    }
}
