//! Command handler registry for IMAP commands
//!
//! This module provides a centralized registry for command handlers,
//! allowing for extensible command processing and clean separation
//! between session management and command logic.

use std::collections::HashMap;
use std::sync::Arc;

use crate::command_handler::CommandHandler;
use crate::error::{Error, Result};
use crate::protocol::Response;
use crate::session_context::SessionContext;

/// Registry of command handlers
///
/// Maintains a mapping from command names to their handlers.
/// Handlers can be registered at startup for built-in commands
/// or dynamically for custom extensions.
#[derive(Clone)]
pub struct CommandHandlers {
    handlers: HashMap<String, Arc<dyn CommandHandler>>,
}

impl CommandHandlers {
    /// Create a new empty command handler registry
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a command handler
    ///
    /// # Arguments
    /// * `handler` - The command handler to register
    ///
    /// # Returns
    /// * `Ok(())` if registration succeeded
    /// * `Err` if a handler with the same name is already registered
    pub fn register(&mut self, handler: Arc<dyn CommandHandler>) -> Result<()> {
        let name = handler.command_name().to_uppercase();

        if self.handlers.contains_key(&name) {
            return Err(Error::Internal(format!(
                "Command handler already registered: {}",
                name
            )));
        }

        self.handlers.insert(name, handler);
        Ok(())
    }

    /// Get a handler by command name
    ///
    /// # Arguments
    /// * `command_name` - The command name (case-insensitive)
    ///
    /// # Returns
    /// * `Some(handler)` if found
    /// * `None` if no handler registered for this command
    pub fn get(&self, command_name: &str) -> Option<Arc<dyn CommandHandler>> {
        self.handlers.get(&command_name.to_uppercase()).cloned()
    }

    /// Handle a command by dispatching to the appropriate handler
    ///
    /// # Arguments
    /// * `command_name` - The command name
    /// * `tag` - The command tag from the client
    /// * `args` - Arguments to the command
    /// * `connection` - Borrowed connection for challenge/response interactions
    /// * `context` - The session context
    /// * `current_state` - The current session state
    ///
    /// # Returns
    /// * Tuple of (Response, Option<SessionState>) from the handler, or a "command not found" error response
    pub async fn handle(
        &self,
        command_name: &str,
        tag: &str,
        args: &str,
        connection: &crate::connection::Connection,
        context: &SessionContext,
        current_state: &crate::session_context::SessionState,
    ) -> (Response, Option<crate::session_context::SessionState>) {
        match self.get(command_name) {
            Some(handler) => {
                // Check authentication requirement
                if handler.requires_auth() {
                    match current_state {
                        crate::session_context::SessionState::NotAuthenticated => {
                            return (Response::No {
                                tag: Some(tag.to_string()),
                                message: "Command requires authentication".to_string(),
                            }, None);
                        }
                        _ => {}
                    }
                }

                // Check selected mailbox requirement
                if handler.requires_selected_mailbox() {
                    match current_state {
                        crate::session_context::SessionState::Selected { .. } => {}
                        _ => {
                            return (Response::No {
                                tag: Some(tag.to_string()),
                                message: "Command requires a selected mailbox".to_string(),
                            }, None);
                        }
                    }
                }

                // Execute the handler
                match handler.handle(tag, args, connection, context, current_state).await {
                    Ok((response, state_update)) => (response, state_update),
                    Err(e) => (Response::Bad {
                        tag: Some(tag.to_string()),
                        message: format!("Handler error: {}", e),
                    }, None),
                }
            }
            None => (Response::Bad {
                tag: Some(tag.to_string()),
                message: format!("Unknown command: {}", command_name),
            }, None),
        }
    }

    /// List all registered command names
    pub fn list_commands(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    /// Check if a command is registered
    pub fn has_command(&self, command_name: &str) -> bool {
        self.handlers.contains_key(&command_name.to_uppercase())
    }
}

impl Default for CommandHandlers {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::protocol::Response;
    use crate::session_context::SessionContext;

    // Mock handler for testing
    struct MockHandler {
        name: String,
        requires_auth: bool,
    }

    #[async_trait]
    impl CommandHandler for MockHandler {
        fn command_name(&self) -> &str {
            &self.name
        }

        async fn handle(
            &self,
            tag: &str,
            _args: &str,
            _connection: &crate::connection::Connection,
            _context: &SessionContext,
            _current_state: &crate::session_context::SessionState,
        ) -> Result<(Response, Option<crate::session_context::SessionState>)> {
            Ok((Response::Ok {
                tag: Some(tag.to_string()),
                message: format!("{} completed", self.name),
            }, None))
        }

        fn requires_auth(&self) -> bool {
            self.requires_auth
        }
    }

    #[test]
    fn test_command_handlers_creation() {
        let handlers = CommandHandlers::new();
        assert_eq!(handlers.list_commands().len(), 0);
    }

    #[test]
    fn test_register_handler() {
        let mut handlers = CommandHandlers::new();
        let handler = Arc::new(MockHandler {
            name: "TEST".to_string(),
            requires_auth: false,
        });

        handlers.register(handler.clone()).unwrap();
        assert!(handlers.has_command("TEST"));
        assert!(handlers.has_command("test")); // Case-insensitive
    }

    #[test]
    fn test_duplicate_registration() {
        let mut handlers = CommandHandlers::new();
        let handler1 = Arc::new(MockHandler {
            name: "TEST".to_string(),
            requires_auth: false,
        });
        let handler2 = Arc::new(MockHandler {
            name: "test".to_string(), // Same name, different case
            requires_auth: false,
        });

        handlers.register(handler1).unwrap();
        let result = handlers.register(handler2);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_handler() {
        let mut handlers = CommandHandlers::new();
        let handler = Arc::new(MockHandler {
            name: "CAPABILITY".to_string(),
            requires_auth: false,
        });

        handlers.register(handler.clone()).unwrap();

        // Case-insensitive lookup
        assert!(handlers.get("CAPABILITY").is_some());
        assert!(handlers.get("capability").is_some());
        assert!(handlers.get("Capability").is_some());
        assert!(handlers.get("UNKNOWN").is_none());
    }

    #[test]
    fn test_list_commands() {
        let mut handlers = CommandHandlers::new();

        handlers.register(Arc::new(MockHandler {
            name: "CAPABILITY".to_string(),
            requires_auth: false,
        })).unwrap();

        handlers.register(Arc::new(MockHandler {
            name: "NOOP".to_string(),
            requires_auth: true,
        })).unwrap();

        let commands = handlers.list_commands();
        assert_eq!(commands.len(), 2);
        assert!(commands.contains(&"CAPABILITY".to_string()));
        assert!(commands.contains(&"NOOP".to_string()));
    }
}
