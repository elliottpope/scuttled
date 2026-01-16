//! NOOP command handler

use async_trait::async_trait;
use crate::command_handler::CommandHandler;
use crate::connection::Connection;
use crate::error::Result;
use crate::protocol::Response;
use crate::session_context::{SessionContext, SessionState};

/// Handler for the NOOP command
///
/// Does nothing successfully. Can be used as a keepalive.
pub struct NoopHandler;

impl NoopHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NoopHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandHandler for NoopHandler {
    fn command_name(&self) -> &str {
        "NOOP"
    }

    async fn handle(
        &self,
        tag: &str,
        _args: &str,
        connection: &Connection,
        _context: &SessionContext,
        _current_state: &SessionState,
    ) -> Result<Option<SessionState>> {
        let response = Response::Ok {
            tag: Some(tag.to_string()),
            message: "NOOP completed".to_string(),
        };
        connection.write_response(&response).await?;
        Ok(None)
    }

    fn requires_auth(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_noop_handler() {
        let handler = NoopHandler::new();
        assert_eq!(handler.command_name(), "NOOP");
        assert!(handler.requires_auth());
    }
}
