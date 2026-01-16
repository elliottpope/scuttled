//! DELETE command handler

use crate::command_handler::CommandHandler;
use crate::connection::Connection;
use crate::error::Result;
use crate::protocol::Response;
use crate::session_context::{SessionContext, SessionState};
use async_trait::async_trait;

/// Handler for the DELETE command
///
/// Deletes a mailbox.
pub struct DeleteHandler;

impl DeleteHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DeleteHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandHandler for DeleteHandler {
    fn command_name(&self) -> &str {
        "DELETE"
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
                message: "DELETE requires a mailbox name".to_string(),
            };
            connection.write_response(&response).await?;
            return Ok(None);
        }

        // Cannot delete INBOX
        if mailbox_name.eq_ignore_ascii_case("INBOX") {
            let response = Response::No {
                tag: Some(tag.to_string()),
                message: "Cannot delete INBOX".to_string(),
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

        // Try to delete the mailbox
        match context
            .mailboxes
            .delete_mailbox(username, mailbox_name)
            .await
        {
            Ok(_) => {
                let response = Response::Ok {
                    tag: Some(tag.to_string()),
                    message: format!("DELETE completed for {}", mailbox_name),
                };
                connection.write_response(&response).await?;
                Ok(None)
            }
            Err(e) => {
                let response = Response::No {
                    tag: Some(tag.to_string()),
                    message: format!("DELETE failed: {}", e),
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
    async fn test_delete_handler() {
        let handler = DeleteHandler::new();
        assert_eq!(handler.command_name(), "DELETE");
        assert!(handler.requires_auth());
    }
}
