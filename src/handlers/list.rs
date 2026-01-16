//! LIST command handler

use crate::command_handler::CommandHandler;
use crate::connection::Connection;
use crate::error::Result;
use crate::protocol::Response;
use crate::session_context::{SessionContext, SessionState};
use async_trait::async_trait;

/// Handler for the LIST command
///
/// Lists mailboxes matching a pattern.
pub struct ListHandler;

impl ListHandler {
    pub fn new() -> Self {
        Self
    }

    /// Parse LIST command arguments
    ///
    /// Format: LIST reference mailbox-pattern
    fn parse_args(args: &str) -> Option<(String, String)> {
        let parts: Vec<&str> = args.split_whitespace().collect();
        if parts.len() >= 2 {
            Some((parts[0].to_string(), parts[1].to_string()))
        } else {
            None
        }
    }
}

impl Default for ListHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandHandler for ListHandler {
    fn command_name(&self) -> &str {
        "LIST"
    }

    async fn handle(
        &self,
        tag: &str,
        args: &str,
        connection: &Connection,
        context: &SessionContext,
        current_state: &SessionState,
    ) -> Result<Option<SessionState>> {
        // Parse arguments
        let (_reference, pattern) = match Self::parse_args(args) {
            Some((r, p)) => (r, p),
            None => {
                let response = Response::Bad {
                    tag: Some(tag.to_string()),
                    message: "Invalid LIST syntax".to_string(),
                };
                connection.write_response(&response).await?;
                return Ok(None);
            }
        };

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

        // List mailboxes
        match context.mailboxes.list_mailboxes(username).await {
            Ok(mailboxes) => {
                // Filter by pattern (simple wildcard matching)
                let filtered: Vec<_> = if pattern == "*" {
                    mailboxes
                } else {
                    // Simple prefix matching for now
                    let prefix = pattern.trim_end_matches('*');
                    mailboxes
                        .into_iter()
                        .filter(|m| m.root_path.starts_with(prefix))
                        .collect()
                };

                // Build response message with list of mailboxes
                let mut message = String::new();
                for mailbox in &filtered {
                    message.push_str(&format!("* LIST () \"/\" \"{}\"\r\n", mailbox.root_path));
                }
                message.push_str(&format!("{} OK LIST completed", tag));

                let response = Response::Ok {
                    tag: Some(tag.to_string()),
                    message,
                };
                connection.write_response(&response).await?;
                Ok(None)
            }
            Err(e) => {
                let response = Response::No {
                    tag: Some(tag.to_string()),
                    message: format!("LIST failed: {}", e),
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

    #[test]
    fn test_parse_args() {
        assert_eq!(
            ListHandler::parse_args("\"\" *"),
            Some(("\"\"".to_string(), "*".to_string()))
        );

        assert_eq!(
            ListHandler::parse_args("\"\" INBOX"),
            Some(("\"\"".to_string(), "INBOX".to_string()))
        );

        assert_eq!(ListHandler::parse_args("*"), None);
        assert_eq!(ListHandler::parse_args(""), None);
    }

    #[tokio::test]
    async fn test_list_handler() {
        let handler = ListHandler::new();
        assert_eq!(handler.command_name(), "LIST");
        assert!(handler.requires_auth());
    }
}
