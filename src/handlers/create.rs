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
        connection: &Connection,
        context: &SessionContext,
        current_state: &SessionState,
    ) -> Result<Option<SessionState>> {
        let mailbox_name = args.trim();

        if mailbox_name.is_empty() {
            let response = Response::Bad {
                tag: Some(tag.to_string()),
                message: "CREATE requires a mailbox name".to_string(),
            };
            connection.write_response(&response).await?;
            return Ok(None);
        }

        // Get current username from session state
        let username = match current_state {
            SessionState::Authenticated { username } => username,
            SessionState::Selected { username, .. } => username,
            _ => {
                let response = Response::No {
                    tag: Some(tag.to_string()),
                    message: "Not authenticated".to_string(),
                };
                connection.write_response(&response).await?;
                return Ok(None);
            }
        };

        // Try to create the mailbox
        match context
            .mailboxes
            .create_mailbox(username, mailbox_name)
            .await
        {
            Ok(_) => {
                let response = Response::Ok {
                    tag: Some(tag.to_string()),
                    message: format!("CREATE completed for {}", mailbox_name),
                };
                connection.write_response(&response).await?;
                Ok(None)
            }
            Err(e) => {
                let response = Response::No {
                    tag: Some(tag.to_string()),
                    message: format!("CREATE failed: {}", e),
                };
                connection.write_response(&response).await?;
                Ok(None)
            }
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
