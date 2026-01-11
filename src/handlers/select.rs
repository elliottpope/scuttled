//! SELECT command handler

use crate::command_handler::CommandHandler;
use crate::error::Result;
use crate::protocol::Response;
use crate::session_context::{SessionContext, SessionState};
use async_trait::async_trait;

/// Handler for the SELECT command
///
/// Selects a mailbox for access.
pub struct SelectHandler;

impl SelectHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SelectHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandHandler for SelectHandler {
    fn command_name(&self) -> &str {
        "SELECT"
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
                message: "SELECT requires a mailbox name".to_string(),
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

        // Check if mailbox exists
        let mailbox = context
            .mailboxes
            .get_mailbox(&username, mailbox_name)
            .await?;

        if mailbox.is_none() {
            return Ok(Response::No {
                tag: Some(tag.to_string()),
                message: format!("Mailbox does not exist: {}", mailbox_name),
            });
        }

        // Update session state to Selected
        context
            .set_state(SessionState::Selected {
                username: username.clone(),
                mailbox: mailbox_name.to_string(),
            })
            .await;

        // Store selected mailbox in context
        context
            .set_selected_mailbox(Some(mailbox_name.to_string()))
            .await;

        Ok(Response::Ok {
            tag: Some(tag.to_string()),
            message: format!("SELECT completed for {}", mailbox_name),
        })
    }

    fn requires_auth(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_select_handler() {
        let handler = SelectHandler::new();
        assert_eq!(handler.command_name(), "SELECT");
        assert!(handler.requires_auth());
    }
}
