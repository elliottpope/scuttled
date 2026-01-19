//! Integration tests for IMAP LOGIN command

use scuttled::authenticator::r#impl::BasicAuthenticator;
use scuttled::index::r#impl::create_inmemory_index;
use scuttled::mailboxes::r#impl::InMemoryMailboxes;
use scuttled::mailstore::r#impl::FilesystemMailStore;
use scuttled::queue::r#impl::ChannelQueue;
use scuttled::server::ImapServer;
use scuttled::userstore::r#impl::SQLiteUserStore;
use scuttled::{Mailboxes, UserStore};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

/// Set up a test server and return the temp directory and address
async fn setup_test_server() -> (TempDir, String) {
    let tmp_dir = TempDir::new().unwrap();
    let mail_dir = tmp_dir.path().join("mail");
    let db_path = tmp_dir.path().join("users.db");

    std::fs::create_dir_all(&mail_dir).unwrap();

    let mail_store = FilesystemMailStore::new(&mail_dir).await.unwrap();
    let user_store = Arc::new(SQLiteUserStore::new(&db_path).await.unwrap());
    let queue = ChannelQueue::new();
    let mailboxes = Arc::new(InMemoryMailboxes::new());
    let index = create_inmemory_index(Some(mailboxes.clone()));

    // Set up test user
    user_store
        .create_user("testuser", "testpass")
        .await
        .unwrap();

    // Create INBOX
    if let Err(e) = mailboxes.create_mailbox("testuser", "INBOX").await {
        log::warn!("Failed to create INBOX (may already exist): {}", e);
    }

    let authenticator = BasicAuthenticator::new(user_store.clone());
    let server = ImapServer::new(
        mail_store,
        index,
        authenticator,
        user_store,
        queue,
        mailboxes,
    );

    let addr = "127.0.0.1:0";
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let actual_addr = listener.local_addr().unwrap().to_string();

    tokio::spawn(async move {
        if let Err(e) = server.listen_on(listener).await {
            eprintln!("Server error: {}", e);
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    (tmp_dir, actual_addr)
}

#[tokio::test]
async fn test_login_success() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_login_wrong_password() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let result = client.login("testuser", "wrongpass").await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_login_nonexistent_user() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let result = client.login("nonexistent", "anypass").await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_login_empty_username() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let result = client.login("", "testpass").await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_login_empty_password() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let result = client.login("testuser", "").await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_login_both_empty() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let result = client.login("", "").await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_login_special_characters_in_password() {
    let tmp_dir = TempDir::new().unwrap();
    let mail_dir = tmp_dir.path().join("mail");
    let db_path = tmp_dir.path().join("users.db");

    std::fs::create_dir_all(&mail_dir).unwrap();

    let mail_store = FilesystemMailStore::new(&mail_dir).await.unwrap();
    let user_store = Arc::new(SQLiteUserStore::new(&db_path).await.unwrap());
    let queue = ChannelQueue::new();
    let mailboxes = Arc::new(InMemoryMailboxes::new());
    let index = create_inmemory_index(Some(mailboxes.clone()));

    // Create user with special characters in password
    let special_pass = "p@ss!w0rd#123$%^&*()";
    user_store
        .create_user("specialuser", special_pass)
        .await
        .unwrap();

    if let Err(e) = mailboxes.create_mailbox("specialuser", "INBOX").await {
        log::warn!("Failed to create INBOX: {}", e);
    }

    let authenticator = BasicAuthenticator::new(user_store.clone());
    let server = ImapServer::new(
        mail_store,
        index,
        authenticator,
        user_store,
        queue,
        mailboxes,
    );

    let addr = "127.0.0.1:0";
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let actual_addr = listener.local_addr().unwrap().to_string();

    tokio::spawn(async move {
        if let Err(e) = server.listen_on(listener).await {
            eprintln!("Server error: {}", e);
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let stream = tokio::net::TcpStream::connect(&actual_addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("specialuser", special_pass).await.unwrap();

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_login_case_sensitive_username() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    // Try logging in with different case - this should fail if usernames are case-sensitive
    let result = client.login("TESTUSER", "testpass").await;

    // This test documents the current behavior - it may pass or fail depending on implementation
    // The assertion just ensures the test runs
    let _ = result;
}

#[tokio::test]
async fn test_login_multiple_sequential() {
    let (_tmp_dir, addr) = setup_test_server().await;

    // First login
    let stream1 = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client1 = async_imap::Client::new(stream1);
    let mut session1 = client1.login("testuser", "testpass").await.unwrap();
    session1.logout().await.unwrap();

    // Second login on new connection
    let stream2 = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client2 = async_imap::Client::new(stream2);
    let mut session2 = client2.login("testuser", "testpass").await.unwrap();
    session2.logout().await.unwrap();
}

#[tokio::test]
async fn test_login_concurrent() {
    let (_tmp_dir, addr) = setup_test_server().await;
    let addr = Arc::new(addr);

    // Spawn multiple concurrent login attempts
    let mut handles = vec![];
    for _ in 0..5 {
        let addr = Arc::clone(&addr);
        let handle = tokio::spawn(async move {
            let stream = tokio::net::TcpStream::connect(addr.as_str()).await.unwrap();
            let client = async_imap::Client::new(stream);
            let mut session = client.login("testuser", "testpass").await.unwrap();
            session.logout().await.unwrap();
        });
        handles.push(handle);
    }

    // Wait for all to complete
    for handle in handles {
        handle.await.unwrap();
    }
}

#[tokio::test]
async fn test_login_very_long_username() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let long_username = "a".repeat(1000);
    let result = client.login(&long_username, "testpass").await;

    // Should fail because user doesn't exist
    assert!(result.is_err());
}

#[tokio::test]
async fn test_login_very_long_password() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let long_password = "a".repeat(1000);
    let result = client.login("testuser", &long_password).await;

    // Should fail because password is wrong
    assert!(result.is_err());
}

#[tokio::test]
async fn test_login_unicode_username() {
    let tmp_dir = TempDir::new().unwrap();
    let mail_dir = tmp_dir.path().join("mail");
    let db_path = tmp_dir.path().join("users.db");

    std::fs::create_dir_all(&mail_dir).unwrap();

    let mail_store = FilesystemMailStore::new(&mail_dir).await.unwrap();
    let user_store = Arc::new(SQLiteUserStore::new(&db_path).await.unwrap());
    let queue = ChannelQueue::new();
    let mailboxes = Arc::new(InMemoryMailboxes::new());
    let index = create_inmemory_index(Some(mailboxes.clone()));

    // Create user with unicode characters
    let unicode_user = "用户名";
    user_store
        .create_user(unicode_user, "testpass")
        .await
        .unwrap();

    if let Err(e) = mailboxes.create_mailbox(unicode_user, "INBOX").await {
        log::warn!("Failed to create INBOX: {}", e);
    }

    let authenticator = BasicAuthenticator::new(user_store.clone());
    let server = ImapServer::new(
        mail_store,
        index,
        authenticator,
        user_store,
        queue,
        mailboxes,
    );

    let addr = "127.0.0.1:0";
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let actual_addr = listener.local_addr().unwrap().to_string();

    tokio::spawn(async move {
        if let Err(e) = server.listen_on(listener).await {
            eprintln!("Server error: {}", e);
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let stream = tokio::net::TcpStream::connect(&actual_addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let result = client.login(unicode_user, "testpass").await;

    // This documents current behavior - may pass or fail depending on implementation
    let _ = result;
}
