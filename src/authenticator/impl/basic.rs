//! Basic authenticator implementation

use async_trait::async_trait;
use std::sync::Arc;

use crate::error::Result;
use crate::authenticator::Authenticator;
use crate::userstore::UserStore;
use crate::types::*;

/// Basic authenticator that uses a UserStore for authentication
pub struct BasicAuthenticator<U: UserStore> {
    pub user_store: Arc<U>,
}

impl<U: UserStore> BasicAuthenticator<U> {
    pub fn new(user_store: U) -> Self {
        Self {
            user_store: Arc::new(user_store),
        }
    }
}

#[async_trait]
impl<U: UserStore + 'static> Authenticator for BasicAuthenticator<U> {
    async fn authenticate(&self, credentials: &Credentials) -> Result<bool> {
        self.user_store
            .verify_password(&credentials.username, &credentials.password)
            .await
    }

    async fn validate_token(&self, _token: &str) -> Result<Option<Username>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::userstore::r#impl::SQLiteUserStore;
    use tempfile::TempDir;

    #[async_std::test]
    async fn test_authenticate_success() {
        let tmp_dir = TempDir::new().unwrap();
        let db_path = tmp_dir.path().join("users.db");
        let user_store = SQLiteUserStore::new(db_path).await.unwrap();

        user_store.create_user("testuser", "password123").await.unwrap();

        let authenticator = BasicAuthenticator::new(user_store);

        let creds = Credentials {
            username: "testuser".to_string(),
            password: "password123".to_string(),
        };

        let result = authenticator.authenticate(&creds).await.unwrap();
        assert!(result);
    }

    #[async_std::test]
    async fn test_authenticate_failure() {
        let tmp_dir = TempDir::new().unwrap();
        let db_path = tmp_dir.path().join("users.db");
        let user_store = SQLiteUserStore::new(db_path).await.unwrap();

        user_store.create_user("testuser", "password123").await.unwrap();

        let authenticator = BasicAuthenticator::new(user_store);

        let creds = Credentials {
            username: "testuser".to_string(),
            password: "wrongpassword".to_string(),
        };

        let result = authenticator.authenticate(&creds).await.unwrap();
        assert!(!result);
    }
}
