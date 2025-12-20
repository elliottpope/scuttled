//! UserStore trait and implementations
//!
//! The UserStore is responsible for managing user accounts and credentials.

use async_trait::async_trait;
use crate::error::Result;
use crate::types::*;

pub mod r#impl;

/// Trait for storing user information
#[async_trait]
pub trait UserStore: Send + Sync {
    /// Create a new user
    async fn create_user(&self, username: &str, password: &str) -> Result<()>;

    /// Get user information
    async fn get_user(&self, username: &str) -> Result<Option<User>>;

    /// Update user password
    async fn update_password(&self, username: &str, new_password: &str) -> Result<()>;

    /// Delete a user
    async fn delete_user(&self, username: &str) -> Result<()>;

    /// List all users
    async fn list_users(&self) -> Result<Vec<Username>>;

    /// Verify a password for a user
    async fn verify_password(&self, username: &str, password: &str) -> Result<bool>;
}
