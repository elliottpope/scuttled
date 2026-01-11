//! CAPABILITY command handler

use async_trait::async_trait;
use crate::command_handler::CommandHandler;
use crate::error::Result;
use crate::protocol::Response;
use crate::session_context::SessionContext;

/// Handler for the CAPABILITY command
///
/// Returns the list of capabilities supported by the server.
pub struct CapabilityHandler {
    is_tls: bool,
}

impl CapabilityHandler {
    pub fn new(is_tls: bool) -> Self {
        Self { is_tls }
    }
}

#[async_trait]
impl CommandHandler for CapabilityHandler {
    fn command_name(&self) -> &str {
        "CAPABILITY"
    }

    async fn handle(
        &self,
        tag: &str,
        _args: &str,
        _context: &mut SessionContext,
    ) -> Result<Response> {
        let mut capabilities = vec!["IMAP4rev1"];

        // Only advertise STARTTLS on non-TLS connections
        if !self.is_tls {
            capabilities.push("STARTTLS");
        }

        Ok(Response::Ok {
            tag: Some(tag.to_string()),
            message: format!("CAPABILITY {}", capabilities.join(" ")),
        })
    }

    fn requires_auth(&self) -> bool {
        false // CAPABILITY can be used before authentication
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::{Authenticator, Index, MailStore, Queue, UserStore};

    #[tokio::test]
    async fn test_capability_handler() {
        let handler = CapabilityHandler::new(false);
        assert_eq!(handler.command_name(), "CAPABILITY");
        assert!(!handler.requires_auth());
    }
}
