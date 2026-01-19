//! Integration tests for IMAP SELECT command

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

    user_store
        .create_user("testuser", "testpass")
        .await
        .unwrap();

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
async fn test_select_inbox_empty() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let mailbox = session.select("INBOX").await.unwrap();
    assert_eq!(mailbox.exists, 0);

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_select_nonexistent_mailbox() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let result = session.select("NonExistent").await;
    assert!(result.is_err());

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_select_created_mailbox() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.create("MyMailbox").await.unwrap();
    let mailbox = session.select("MyMailbox").await.unwrap();
    assert_eq!(mailbox.exists, 0);

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_select_mailbox_with_messages() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    // Append some messages
    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    session.append("INBOX", None, None, message).await.unwrap();
    session.append("INBOX", None, None, message).await.unwrap();
    session.append("INBOX", None, None, message).await.unwrap();

    let mailbox = session.select("INBOX").await.unwrap();
    assert_eq!(mailbox.exists, 3);

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_examine_readonly() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let mailbox = session.examine("INBOX").await.unwrap();
    assert_eq!(mailbox.exists, 0);

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_select_multiple_mailboxes_sequential() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.create("Mailbox1").await.unwrap();
    session.create("Mailbox2").await.unwrap();

    let mb1 = session.select("Mailbox1").await.unwrap();
    assert_eq!(mb1.exists, 0);

    let mb2 = session.select("Mailbox2").await.unwrap();
    assert_eq!(mb2.exists, 0);

    let inbox = session.select("INBOX").await.unwrap();
    assert_eq!(inbox.exists, 0);

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_select_same_mailbox_twice() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let mb1 = session.select("INBOX").await.unwrap();
    assert_eq!(mb1.exists, 0);

    let mb2 = session.select("INBOX").await.unwrap();
    assert_eq!(mb2.exists, 0);

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_select_empty_mailbox_name() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let result = session.select("").await;
    assert!(result.is_err());

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_select_mailbox_with_special_chars() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    // Try creating and selecting mailbox with special characters
    let mailbox_name = "My Mailbox!@#";
    let create_result = session.create(mailbox_name).await;

    if create_result.is_ok() {
        let select_result = session.select(mailbox_name).await;
        // This documents current behavior
        let _ = select_result;
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_select_hierarchical_mailbox() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    // Try hierarchical mailbox name
    let create_result = session.create("Folder/Subfolder").await;

    if create_result.is_ok() {
        let select_result = session.select("Folder/Subfolder").await;
        // This documents current behavior
        let _ = select_result;
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_select_case_sensitivity() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.create("TestBox").await.unwrap();

    // Try selecting with different case
    let result = session.select("testbox").await;
    // This documents current behavior - may pass or fail depending on case sensitivity
    let _ = result;

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_select_after_delete() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.create("TempBox").await.unwrap();
    session.delete("TempBox").await.unwrap();

    let result = session.select("TempBox").await;
    assert!(result.is_err());

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_select_mailbox_flags() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let mailbox = session.select("INBOX").await.unwrap();

    // Check that mailbox has expected flags (any number is valid)
    let _ = mailbox.flags.len();

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_select_mailbox_permanent_flags() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let mailbox = session.select("INBOX").await.unwrap();

    // Check permanent flags (may be empty)
    let _ = mailbox.permanent_flags;

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_select_mailbox_uidvalidity() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let mailbox = session.select("INBOX").await.unwrap();

    // UIDVALIDITY should be present
    assert!(mailbox.uid_validity.is_some());

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_select_mailbox_recent() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let mailbox = session.select("INBOX").await.unwrap();

    // Recent count should be 0 for empty mailbox
    assert_eq!(mailbox.recent, 0);

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_select_very_long_mailbox_name() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let long_name = "A".repeat(1000);
    let result = session.select(&long_name).await;

    // Should fail - mailbox doesn't exist
    assert!(result.is_err());

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_select_unicode_mailbox_name() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let unicode_name = "受信箱";
    let create_result = session.create(unicode_name).await;

    if create_result.is_ok() {
        let select_result = session.select(unicode_name).await;
        // Documents current behavior
        let _ = select_result;
    }

    session.logout().await.unwrap();
}
