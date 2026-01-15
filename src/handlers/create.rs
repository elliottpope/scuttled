//! CREATE command handler

use crate::command_handler::CommandHandler;
use crate::connection::Connection;
use crate::error::Result;
use crate::protocol::Response;
use crate::session_context::{SessionContext, SessionState};
use async_trait::async_trait;

/// Handler for the CREATE command
///
/// Creates a new mailbox.
pub struct CreateHandler;

impl CreateHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CreateHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandHandler for CreateHandler {
    fn command_name(&self) -> &str {
        "CREATE"
    }

    async fn handle(
        &self,
        tag: &str,
        args: &str,
        _connection: &Connection,
        context: &SessionContext,
        current_state: &SessionState,
    ) -> Result<(Response, Option<SessionState>)> {
        let mailbox_name = args.trim();

        if mailbox_name.is_empty() {
            return Ok((Response::Bad {
                tag: Some(tag.to_string()),
                message: "CREATE requires a mailbox name".to_string(),
            }, None));
        }

        // Get current username from session state
        let username = match current_state {
            SessionState::Authenticated { username } => username,
            SessionState::Selected { username, .. } => username,
            _ => {
                return Ok((Response::No {
                    tag: Some(tag.to_string()),
                    message: "Not authenticated".to_string(),
                }, None));
            }
        };

        // Try to create the mailbox
        match context
            .mailboxes
            .create_mailbox(username, mailbox_name)
            .await
        {
            Ok(_) => Ok((Response::Ok {
                tag: Some(tag.to_string()),
                message: format!("CREATE completed for {}", mailbox_name),
            }, None)),
            Err(e) => Ok((Response::No {
                tag: Some(tag.to_string()),
                message: format!("CREATE failed: {}", e),
            }, None)),
        }
    }

    fn requires_auth(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_handler() {
        let handler = CreateHandler::new();
        assert_eq!(handler.command_name(), "CREATE");
        assert!(handler.requires_auth());
    }
}
