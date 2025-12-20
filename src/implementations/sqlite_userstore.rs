//! SQLite-based user store implementation

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::error::{Error, Result};
use crate::traits::UserStore;
use crate::types::*;

/// SQLite-based user store
pub struct SQLiteUserStore {
    db_path: Arc<PathBuf>,
}

impl SQLiteUserStore {
    /// Create a new SQLite user store
    pub async fn new<P: AsRef<Path>>(db_path: P) -> Result<Self> {
        let db_path = db_path.as_ref().to_path_buf();
        let db_path_clone = db_path.clone();

        async_std::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path_clone)?;
            conn.execute(
                "CREATE TABLE IF NOT EXISTS users (
                    username TEXT PRIMARY KEY,
                    password_hash TEXT NOT NULL,
                    created_at TEXT NOT NULL
                )",
                [],
            )?;
            Ok::<(), Error>(())
        })
        .await?;

        Ok(Self {
            db_path: Arc::new(db_path),
        })
    }

    /// Create an in-memory user store for testing
    pub async fn in_memory() -> Result<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let db_path = PathBuf::from(format!("file:memdb{}?mode=memory&cache=shared", id));

        let db_path_clone = db_path.clone();
        async_std::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path_clone)?;
            conn.execute(
                "CREATE TABLE IF NOT EXISTS users (
                    username TEXT PRIMARY KEY,
                    password_hash TEXT NOT NULL,
                    created_at TEXT NOT NULL
                )",
                [],
            )?;
            Ok::<(), Error>(())
        })
        .await?;

        Ok(Self {
            db_path: Arc::new(db_path),
        })
    }
}

#[async_trait]
impl UserStore for SQLiteUserStore {
    async fn create_user(&self, username: &str, password: &str) -> Result<()> {
        let username = username.to_string();
        let password = password.to_string();
        let db_path = Arc::clone(&self.db_path);

        async_std::task::spawn_blocking(move || {
            let password_hash = bcrypt::hash(&password, bcrypt::DEFAULT_COST)
                .map_err(|e| Error::Internal(format!("Failed to hash password: {}", e)))?;

            let created_at = Utc::now().to_rfc3339();

            let conn = Connection::open(&*db_path)?;
            conn.execute(
                "INSERT INTO users (username, password_hash, created_at) VALUES (?1, ?2, ?3)",
                params![username, password_hash, created_at],
            )
            .map_err(|e| {
                if e.to_string().contains("UNIQUE constraint failed") {
                    Error::AlreadyExists(format!("User already exists: {}", username))
                } else {
                    Error::from(e)
                }
            })?;

            Ok(())
        })
        .await
    }

    async fn get_user(&self, username: &str) -> Result<Option<User>> {
        let username = username.to_string();
        let db_path = Arc::clone(&self.db_path);

        async_std::task::spawn_blocking(move || {
            let conn = Connection::open(&*db_path)?;
            let mut stmt = conn
                .prepare("SELECT username, password_hash, created_at FROM users WHERE username = ?1")?;

            let mut rows = stmt.query(params![username])?;

            if let Some(row) = rows.next()? {
                let username: String = row.get(0)?;
                let password_hash: String = row.get(1)?;
                let created_at_str: String = row.get(2)?;

                let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
                    .map_err(|e| Error::Internal(format!("Failed to parse date: {}", e)))?
                    .with_timezone(&Utc);

                Ok(Some(User {
                    username,
                    password_hash,
                    created_at,
                }))
            } else {
                Ok(None)
            }
        })
        .await
    }

    async fn update_password(&self, username: &str, new_password: &str) -> Result<()> {
        let username = username.to_string();
        let new_password = new_password.to_string();
        let db_path = Arc::clone(&self.db_path);

        async_std::task::spawn_blocking(move || {
            let password_hash = bcrypt::hash(&new_password, bcrypt::DEFAULT_COST)
                .map_err(|e| Error::Internal(format!("Failed to hash password: {}", e)))?;

            let conn = Connection::open(&*db_path)?;
            let affected = conn.execute(
                "UPDATE users SET password_hash = ?1 WHERE username = ?2",
                params![password_hash, username],
            )?;

            if affected == 0 {
                return Err(Error::NotFound(format!("User not found: {}", username)));
            }

            Ok(())
        })
        .await
    }

    async fn delete_user(&self, username: &str) -> Result<()> {
        let username = username.to_string();
        let db_path = Arc::clone(&self.db_path);

        async_std::task::spawn_blocking(move || {
            let conn = Connection::open(&*db_path)?;
            let affected = conn.execute("DELETE FROM users WHERE username = ?1", params![username])?;

            if affected == 0 {
                return Err(Error::NotFound(format!("User not found: {}", username)));
            }

            Ok(())
        })
        .await
    }

    async fn list_users(&self) -> Result<Vec<Username>> {
        let db_path = Arc::clone(&self.db_path);

        async_std::task::spawn_blocking(move || {
            let conn = Connection::open(&*db_path)?;
            let mut stmt = conn.prepare("SELECT username FROM users")?;

            let users = stmt
                .query_map([], |row| row.get(0))?
                .collect::<std::result::Result<Vec<String>, _>>()?;

            Ok(users)
        })
        .await
    }

    async fn verify_password(&self, username: &str, password: &str) -> Result<bool> {
        if let Some(user) = self.get_user(username).await? {
            let password = password.to_string();
            async_std::task::spawn_blocking(move || {
                Ok(bcrypt::verify(&password, &user.password_hash)
                    .map_err(|e| Error::Internal(format!("Failed to verify password: {}", e)))?)
            })
            .await
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[async_std::test]
    async fn test_create_and_get_user() {
        let tmpfile = NamedTempFile::new().unwrap();
        let store = SQLiteUserStore::new(tmpfile.path()).await.unwrap();

        store.create_user("testuser", "password123").await.unwrap();

        let user = store.get_user("testuser").await.unwrap();
        assert!(user.is_some());
        assert_eq!(user.unwrap().username, "testuser");
    }

    #[async_std::test]
    async fn test_verify_password() {
        let tmpfile = NamedTempFile::new().unwrap();
        let store = SQLiteUserStore::new(tmpfile.path()).await.unwrap();

        store.create_user("testuser", "password123").await.unwrap();

        let valid = store.verify_password("testuser", "password123").await.unwrap();
        assert!(valid);

        let invalid = store.verify_password("testuser", "wrongpassword").await.unwrap();
        assert!(!invalid);
    }

    #[async_std::test]
    async fn test_update_password() {
        let tmpfile = NamedTempFile::new().unwrap();
        let store = SQLiteUserStore::new(tmpfile.path()).await.unwrap();

        store.create_user("testuser", "password123").await.unwrap();
        store.update_password("testuser", "newpassword").await.unwrap();

        let valid = store.verify_password("testuser", "newpassword").await.unwrap();
        assert!(valid);

        let old_invalid = store.verify_password("testuser", "password123").await.unwrap();
        assert!(!old_invalid);
    }

    #[async_std::test]
    async fn test_delete_user() {
        let tmpfile = NamedTempFile::new().unwrap();
        let store = SQLiteUserStore::new(tmpfile.path()).await.unwrap();

        store.create_user("testuser", "password123").await.unwrap();
        store.delete_user("testuser").await.unwrap();

        let user = store.get_user("testuser").await.unwrap();
        assert!(user.is_none());
    }

    #[async_std::test]
    async fn test_list_users() {
        let tmpfile = NamedTempFile::new().unwrap();
        let store = SQLiteUserStore::new(tmpfile.path()).await.unwrap();

        store.create_user("user1", "password1").await.unwrap();
        store.create_user("user2", "password2").await.unwrap();
        store.create_user("user3", "password3").await.unwrap();

        let users = store.list_users().await.unwrap();
        assert_eq!(users.len(), 3);
        assert!(users.contains(&"user1".to_string()));
        assert!(users.contains(&"user2".to_string()));
        assert!(users.contains(&"user3".to_string()));
    }
}
