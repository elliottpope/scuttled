//! LOGIN command handler

use async_trait::async_trait;
use crate::command_handler::CommandHandler;
use crate::error::Result;
use crate::protocol::Response;
use crate::session_context::{SessionContext, SessionState};
use crate::types::Credentials;

/// Handler for the LOGIN command
///
/// Authenticates a user with username and password.
pub struct LoginHandler;

impl LoginHandler {
    pub fn new() -> Self {
        Self
    }

    /// Parse LOGIN command arguments
    ///
    /// Format: LOGIN username password
    fn parse_args(args: &str) -> Option<(String, String)> {
        let parts: Vec<&str> = args.split_whitespace().collect();
        if parts.len() == 2 {
            Some((parts[0].to_string(), parts[1].to_string()))
        } else {
            None
        }
    }
}

impl Default for LoginHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandHandler for LoginHandler {
    fn command_name(&self) -> &str {
        "LOGIN"
    }

    async fn handle(
        &self,
        tag: &str,
        args: &str,
        context: &mut SessionContext,
    ) -> Result<Response> {
        // Parse username and password
        let (username, password) = match Self::parse_args(args) {
            Some((u, p)) => (u, p),
            None => {
                return Ok(Response::Bad {
                    tag: Some(tag.to_string()),
                    message: "Invalid LOGIN syntax".to_string(),
                });
            }
        };

        // Authenticate
        let credentials = Credentials {
            username: username.clone(),
            password,
        };
        let auth_result = context.authenticator.authenticate(&credentials).await;

        match auth_result {
            Ok(true) => {
                // Set session state to authenticated
                context
                    .set_state(SessionState::Authenticated { username })
                    .await;

                Ok(Response::Ok {
                    tag: Some(tag.to_string()),
                    message: "LOGIN completed".to_string(),
                })
            }
            Ok(false) => Ok(Response::No {
                tag: Some(tag.to_string()),
                message: "LOGIN failed: invalid credentials".to_string(),
            }),
            Err(e) => Ok(Response::No {
                tag: Some(tag.to_string()),
                message: format!("LOGIN failed: {}", e),
            }),
        }
    }

    fn requires_auth(&self) -> bool {
        false // LOGIN is used to authenticate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_args() {
        assert_eq!(
            LoginHandler::parse_args("alice password123"),
            Some(("alice".to_string(), "password123".to_string()))
        );

        assert_eq!(LoginHandler::parse_args("alice"), None);
        assert_eq!(LoginHandler::parse_args(""), None);
    }

    #[tokio::test]
    async fn test_login_handler() {
        let handler = LoginHandler::new();
        assert_eq!(handler.command_name(), "LOGIN");
        assert!(!handler.requires_auth());
    }
}
