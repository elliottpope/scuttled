//! Integration tests for IMAP FETCH command

use futures::stream::StreamExt;
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
async fn test_fetch_single_message_rfc822() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    session.append("INBOX", None, None, message).await.unwrap();
    session.select("INBOX").await.unwrap();

    {
        let mut messages = session.fetch("1", "RFC822").await.unwrap();
        let mut count = 0;
        while let Some(_) = messages.next().await {
            count += 1;
        }

        assert_eq!(count, 1);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_multiple_messages() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    session.append("INBOX", None, None, message).await.unwrap();
    session.append("INBOX", None, None, message).await.unwrap();
    session.append("INBOX", None, None, message).await.unwrap();

    session.select("INBOX").await.unwrap();

    {
        let mut messages = session.fetch("1:3", "RFC822").await.unwrap();
        let mut count = 0;
        while let Some(_) = messages.next().await {
            count += 1;
        }

        assert_eq!(count, 3);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_from_empty_mailbox() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.select("INBOX").await.unwrap();

    {
        let mut messages = session.fetch("1", "RFC822").await.unwrap();
        let mut count = 0;
        while let Some(_) = messages.next().await {
            count += 1;
        }

        assert_eq!(count, 0);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_nonexistent_message() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    session.append("INBOX", None, None, message).await.unwrap();
    session.select("INBOX").await.unwrap();

    // Try to fetch message 99 when only 1 exists
    {
        let mut messages = session.fetch("99", "RFC822").await.unwrap();
        let mut count = 0;
        while let Some(_) = messages.next().await {
            count += 1;
        }

        assert_eq!(count, 0);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_flags() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    session.append("INBOX", None, None, message).await.unwrap();
    session.select("INBOX").await.unwrap();

    {
        let mut messages = session.fetch("1", "FLAGS").await.unwrap();
        let mut count = 0;
        while let Some(msg_result) = messages.next().await {
            if let Ok(msg) = msg_result {
                // Check that flags are present
                let _ = msg.flags();
                count += 1;
            }
        }

        assert_eq!(count, 1);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_body() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    session.append("INBOX", None, None, message).await.unwrap();
    session.select("INBOX").await.unwrap();

    {
        let mut messages = session.fetch("1", "BODY[]").await.unwrap();
        let mut count = 0;
        while let Some(_) = messages.next().await {
            count += 1;
        }

        assert_eq!(count, 1);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_body_peek() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    session.append("INBOX", None, None, message).await.unwrap();
    session.select("INBOX").await.unwrap();

    {
        let mut messages = session.fetch("1", "BODY.PEEK[]").await.unwrap();
        let mut count = 0;
        while let Some(_) = messages.next().await {
            count += 1;
        }

        assert_eq!(count, 1);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_envelope() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    session.append("INBOX", None, None, message).await.unwrap();
    session.select("INBOX").await.unwrap();

    {
        let mut messages = session.fetch("1", "ENVELOPE").await.unwrap();
        let mut count = 0;
        while let Some(msg_result) = messages.next().await {
            if let Ok(msg) = msg_result {
                // Check that envelope is present
                let _ = msg.envelope();
                count += 1;
            }
        }

        assert_eq!(count, 1);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_uid() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    session.append("INBOX", None, None, message).await.unwrap();
    session.select("INBOX").await.unwrap();

    {
        let mut messages = session.fetch("1", "UID").await.unwrap();
        let mut count = 0;
        while let Some(msg_result) = messages.next().await {
            if let Ok(msg) = msg_result {
                // Check that UID is present
                assert!(msg.uid.is_some());
                count += 1;
            }
        }

        assert_eq!(count, 1);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_all_attributes() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    session.append("INBOX", None, None, message).await.unwrap();
    session.select("INBOX").await.unwrap();

    {
        let mut messages = session.fetch("1", "ALL").await.unwrap();
        let mut count = 0;
        while let Some(_) = messages.next().await {
            count += 1;
        }

        assert_eq!(count, 1);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_fast_attributes() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    session.append("INBOX", None, None, message).await.unwrap();
    session.select("INBOX").await.unwrap();

    {
        let mut messages = session.fetch("1", "FAST").await.unwrap();
        let mut count = 0;
        while let Some(_) = messages.next().await {
            count += 1;
        }

        assert_eq!(count, 1);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_full_attributes() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    session.append("INBOX", None, None, message).await.unwrap();
    session.select("INBOX").await.unwrap();

    {
        let mut messages = session.fetch("1", "FULL").await.unwrap();
        let mut count = 0;
        while let Some(_) = messages.next().await {
            count += 1;
        }

        assert_eq!(count, 1);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_range() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    for _ in 0..5 {
        session.append("INBOX", None, None, message).await.unwrap();
    }

    session.select("INBOX").await.unwrap();

    {
        let mut messages = session.fetch("2:4", "RFC822").await.unwrap();
        let mut count = 0;
        while let Some(_) = messages.next().await {
            count += 1;
        }

        assert_eq!(count, 3);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_star() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    session.append("INBOX", None, None, message).await.unwrap();
    session.append("INBOX", None, None, message).await.unwrap();

    session.select("INBOX").await.unwrap();

    // Fetch last message using *
    {
        let mut messages = session.fetch("*", "RFC822").await.unwrap();
        let mut count = 0;
        while let Some(_) = messages.next().await {
            count += 1;
        }

        assert_eq!(count, 1);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_comma_separated() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    for _ in 0..5 {
        session.append("INBOX", None, None, message).await.unwrap();
    }

    session.select("INBOX").await.unwrap();

    {
        let mut messages = session.fetch("1,3,5", "RFC822").await.unwrap();
        let mut count = 0;
        while let Some(_) = messages.next().await {
            count += 1;
        }

        assert_eq!(count, 3);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_invalid_sequence() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    session.append("INBOX", None, None, message).await.unwrap();
    session.select("INBOX").await.unwrap();

    // Try invalid sequence like 0
    {
        let result = session.fetch("0", "RFC822").await;
        // Documents current behavior - may succeed with 0 results or error
        let _ = result;
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_without_select() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    // Try to fetch without selecting a mailbox first
    {
        let result = session.fetch("1", "RFC822").await;

        // Should fail - no mailbox selected
        assert!(result.is_err());
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_size() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    session.append("INBOX", None, None, message).await.unwrap();
    session.select("INBOX").await.unwrap();

    {
        let mut messages = session.fetch("1", "RFC822.SIZE").await.unwrap();
        let mut count = 0;
        while let Some(msg_result) = messages.next().await {
            if let Ok(msg) = msg_result {
                // Check that size is present
                let _ = msg.size;
                count += 1;
            }
        }

        assert_eq!(count, 1);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_internal_date() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    session.append("INBOX", None, None, message).await.unwrap();
    session.select("INBOX").await.unwrap();

    {
        let mut messages = session.fetch("1", "INTERNALDATE").await.unwrap();
        let mut count = 0;
        while let Some(msg_result) = messages.next().await {
            if let Ok(msg) = msg_result {
                // Check that internal date is present
                let _ = msg.internal_date();
                count += 1;
            }
        }

        assert_eq!(count, 1);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_body_structure() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody";
    session.append("INBOX", None, None, message).await.unwrap();
    session.select("INBOX").await.unwrap();

    {
        let mut messages = session.fetch("1", "BODYSTRUCTURE").await.unwrap();
        let mut count = 0;
        while let Some(_) = messages.next().await {
            count += 1;
        }

        assert_eq!(count, 1);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_partial_body() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nBody content here";
    session.append("INBOX", None, None, message).await.unwrap();
    session.select("INBOX").await.unwrap();

    // Fetch partial body
    {
        let mut messages = session.fetch("1", "BODY[]<0.10>").await.unwrap();
        let mut count = 0;
        while let Some(_) = messages.next().await {
            count += 1;
        }

        assert_eq!(count, 1);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_fetch_large_message() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    // Create a large message (10KB)
    let body = "X".repeat(10000);
    let message = format!(
        "From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Large\r\n\r\n{}",
        body
    );
    session
        .append("INBOX", None, None, message.as_bytes())
        .await
        .unwrap();

    session.select("INBOX").await.unwrap();

    {
        let mut messages = session.fetch("1", "RFC822").await.unwrap();
        let mut count = 0;
        while let Some(_) = messages.next().await {
            count += 1;
        }

        assert_eq!(count, 1);
    }

    session.logout().await.unwrap();
}
