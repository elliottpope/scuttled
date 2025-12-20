//! Authenticator trait and implementations
//!
//! The Authenticator is responsible for verifying user credentials.

use async_trait::async_trait;
use crate::error::Result;
use crate::types::*;

pub mod r#impl;

/// Trait for authenticating IMAP connections
#[async_trait]
pub trait Authenticator: Send + Sync {
    /// Authenticate a user with credentials
    async fn authenticate(&self, credentials: &Credentials) -> Result<bool>;

    /// Validate an authentication token (for future OAUTH support)
    async fn validate_token(&self, token: &str) -> Result<Option<Username>>;
}
