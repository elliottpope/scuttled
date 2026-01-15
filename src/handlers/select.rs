//! SELECT command handler

use crate::command_handler::CommandHandler;
use crate::connection::Connection;
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
        _connection: &Connection,
        context: &SessionContext,
        current_state: &SessionState,
    ) -> Result<(Response, Option<SessionState>)> {
        let mailbox_name = args.trim();

        if mailbox_name.is_empty() {
            return Ok((Response::Bad {
                tag: Some(tag.to_string()),
                message: "SELECT requires a mailbox name".to_string(),
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

        // Check if mailbox exists
        let mailbox = context
            .mailboxes
            .get_mailbox(username, mailbox_name)
            .await?;

        if mailbox.is_none() {
            return Ok((Response::No {
                tag: Some(tag.to_string()),
                message: format!("Mailbox does not exist: {}", mailbox_name),
            }, None));
        }

        // Return new Selected state
        Ok((Response::Ok {
            tag: Some(tag.to_string()),
            message: format!("SELECT completed for {}", mailbox_name),
        }, Some(SessionState::Selected {
            username: username.clone(),
            mailbox: mailbox_name.to_string(),
        })))
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
