//! Command handler trait for extensible IMAP commands

use async_trait::async_trait;
use crate::error::Result;
use crate::protocol::Response;
use crate::session_context::SessionContext;

/// Trait for handling IMAP commands
///
/// Library users can implement this trait to add custom commands to the server.
#[async_trait]
pub trait CommandHandler: Send + Sync {
    /// The name of the command this handler processes (e.g., "CAPABILITY", "MYCUSTOMCMD")
    fn command_name(&self) -> &str;

    /// Handle the command and return a response
    ///
    /// # Arguments
    /// * `tag` - The command tag from the client
    /// * `args` - Arguments to the command (everything after the command name)
    /// * `context` - The session context with access to stores and state
    async fn handle(
        &self,
        tag: &str,
        args: &str,
        context: &mut SessionContext,
    ) -> Result<Response>;

    /// Whether this command requires authentication
    /// Default: true (most commands require auth)
    fn requires_auth(&self) -> bool {
        true
    }

    /// Whether this command requires a selected mailbox
    /// Default: false (only some commands like FETCH need this)
    fn requires_selected_mailbox(&self) -> bool {
        false
    }
}
