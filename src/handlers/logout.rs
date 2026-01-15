//! LOGOUT command handler

use async_trait::async_trait;
use crate::command_handler::CommandHandler;
use crate::connection::Connection;
use crate::error::Result;
use crate::protocol::Response;
use crate::session_context::{SessionContext, SessionState};

/// Handler for the LOGOUT command
///
/// Closes the connection gracefully.
pub struct LogoutHandler;

impl LogoutHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LogoutHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandHandler for LogoutHandler {
    fn command_name(&self) -> &str {
        "LOGOUT"
    }

    async fn handle(
        &self,
        tag: &str,
        _args: &str,
        _connection: &Connection,
        _context: &SessionContext,
        _current_state: &SessionState,
    ) -> Result<(Response, Option<SessionState>)> {
        // Return Logout state
        Ok((Response::Ok {
            tag: Some(tag.to_string()),
            message: "LOGOUT completed".to_string(),
        }, Some(SessionState::Logout)))
    }

    fn requires_auth(&self) -> bool {
        false // LOGOUT can be used anytime
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_logout_handler() {
        let handler = LogoutHandler::new();
        assert_eq!(handler.command_name(), "LOGOUT");
        assert!(!handler.requires_auth());
    }
}
