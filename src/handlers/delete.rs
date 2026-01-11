//! DELETE command handler

use crate::command_handler::CommandHandler;
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
        context: &mut SessionContext,
    ) -> Result<Response> {
        let mailbox_name = args.trim();

        if mailbox_name.is_empty() {
            return Ok(Response::Bad {
                tag: Some(tag.to_string()),
                message: "DELETE requires a mailbox name".to_string(),
            });
        }

        // Cannot delete INBOX
        if mailbox_name.eq_ignore_ascii_case("INBOX") {
            return Ok(Response::No {
                tag: Some(tag.to_string()),
                message: "Cannot delete INBOX".to_string(),
            });
        }

        // Get current username from session state
        let username = match context.get_state().await {
            SessionState::Authenticated { username } => username,
            SessionState::Selected { username, .. } => username,
            _ => {
                return Ok(Response::No {
                    tag: Some(tag.to_string()),
                    message: "Not authenticated".to_string(),
                });
            }
        };

        // Try to delete the mailbox
        match context
            .mailboxes
            .delete_mailbox(&username, mailbox_name)
            .await
        {
            Ok(_) => Ok(Response::Ok {
                tag: Some(tag.to_string()),
                message: format!("DELETE completed for {}", mailbox_name),
            }),
            Err(e) => Ok(Response::No {
                tag: Some(tag.to_string()),
                message: format!("DELETE failed: {}", e),
            }),
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
