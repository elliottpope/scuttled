//! Integration tests for the IMAP server using external IMAP client

use scuttled::authenticator::r#impl::BasicAuthenticator;
use scuttled::index::r#impl::InMemoryIndex;
use scuttled::index::IndexedMessage;
use scuttled::mailboxes::r#impl::InMemoryMailboxes;
use scuttled::mailstore::r#impl::FilesystemMailStore;
use scuttled::queue::r#impl::ChannelQueue;
use scuttled::server::ImapServer;
use scuttled::types::*;
use scuttled::userstore::r#impl::SQLiteUserStore;
use scuttled::{Authenticator, Index, MailStore, Queue, UserStore};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use futures::stream::StreamExt;

/// Set up a test server and return the temp directory and address
async fn setup_test_server() -> (TempDir, String) {
    let tmp_dir = TempDir::new().unwrap();
    let mail_dir = tmp_dir.path().join("mail");
    let db_path = tmp_dir.path().join("users.db");

    std::fs::create_dir_all(&mail_dir).unwrap();

    // Create components using new architecture
    let mail_store = FilesystemMailStore::new(&mail_dir).await.unwrap();
    let index = InMemoryIndex::new();
    let user_store = Arc::new(SQLiteUserStore::new(&db_path).await.unwrap());
    let queue = ChannelQueue::new();
    let mailboxes = InMemoryMailboxes::new();

    // Set up test user and mailbox
    user_store
        .create_user("testuser", "testpass")
        .await
        .unwrap();
    index.create_mailbox("testuser", "INBOX").await.unwrap();

    let authenticator = BasicAuthenticator::new(user_store.clone());
    let server = ImapServer::new(mail_store, index, authenticator, user_store, queue, mailboxes);

    // Bind to a random port
    let addr = "127.0.0.1:0";
    let listener = async_std::net::TcpListener::bind(addr).await.unwrap();
    let actual_addr = listener.local_addr().unwrap().to_string();

    // Spawn server in background
    async_std::task::spawn(async move {
        if let Err(e) = server.listen_on(listener).await {
            eprintln!("Server error: {}", e);
        }
    });

    // Give server time to start
    async_std::task::sleep(Duration::from_millis(100)).await;

    (tmp_dir, actual_addr)
}

#[tokio::test]
async fn test_imap_login() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = async_std::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_imap_login_failure() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = async_std::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let result = client.login("testuser", "wrongpass").await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_imap_select_mailbox() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = async_std::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let mailbox = session.select("INBOX").await.unwrap();
    assert_eq!(mailbox.exists, 0);

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_imap_create_mailbox() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = async_std::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.create("Drafts").await.unwrap();

    {
        let mut mailboxes = session.list(None, Some("*")).await.unwrap();
        let mut mailbox_names = Vec::new();
        while let Some(name_result) = mailboxes.next().await {
            let name = name_result.unwrap();
            mailbox_names.push(name.name().to_string());
        }

        assert!(mailbox_names.contains(&"Drafts".to_string()));
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_imap_list_mailboxes() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = async_std::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    // Create a few mailboxes
    session.create("Sent").await.unwrap();
    session.create("Trash").await.unwrap();

    {
        let mut mailboxes = session.list(None, Some("*")).await.unwrap();
        let mut count = 0;
        while let Some(_) = mailboxes.next().await {
            count += 1;
        }
        assert!(count >= 2); // At least Sent and Trash (INBOX may or may not be listed)
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_imap_delete_mailbox() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = async_std::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.create("ToDelete").await.unwrap();
    session.delete("ToDelete").await.unwrap();

    {
        let mut mailboxes = session.list(None, Some("*")).await.unwrap();
        let mut mailbox_names = Vec::new();
        while let Some(name_result) = mailboxes.next().await {
            let name = name_result.unwrap();
            mailbox_names.push(name.name().to_string());
        }

        assert!(!mailbox_names.contains(&"ToDelete".to_string()));
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_imap_append_message() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = async_std::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.select("INBOX").await.unwrap();

    let message = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test\r\n\r\nThis is a test message.";
    session.append("INBOX", message).await.unwrap();

    // Check that the message was added
    let mailbox = session.examine("INBOX").await.unwrap();
    assert_eq!(mailbox.exists, 1);

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_imap_fetch_message() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = async_std::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    // Append a message first
    let message_content = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: Test Fetch\r\n\r\nTest body.";
    session
        .append("INBOX", message_content)
        .await
        .unwrap();

    session.select("INBOX").await.unwrap();

    // Fetch the message
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
async fn test_imap_expunge() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = async_std::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    // Append two messages
    session
        .append("INBOX", b"Message 1")
        .await
        .unwrap();
    session
        .append("INBOX", b"Message 2")
        .await
        .unwrap();

    session.select("INBOX").await.unwrap();

    // Mark first message as deleted
    {
        let mut store_result = session.store("1", "+FLAGS (\\Deleted)").await.unwrap();
        while let Some(_) = store_result.next().await {}
    }

    // Expunge
    {
        let expunge_result = session.expunge().await.unwrap();
        futures::pin_mut!(expunge_result);
        while let Some(_) = expunge_result.next().await {}
    }

    // Check that only one message remains
    let mailbox = session.examine("INBOX").await.unwrap();
    assert_eq!(mailbox.exists, 1);

    session.logout().await.unwrap();
}

// Component-level integration tests (not using IMAP protocol)

#[async_std::test]
async fn test_mailstore_path_based_operations() {
    let tmp_dir = TempDir::new().unwrap();
    let mail_dir = tmp_dir.path().join("mail");
    let mail_store = FilesystemMailStore::new(&mail_dir).await.unwrap();

    let path = "testuser/INBOX/msg123.eml";
    let content = b"From: test@example.com\r\nSubject: Test\r\n\r\nBody";

    // Store message
    mail_store.store(path, content).await.unwrap();

    // Check it exists
    let exists = mail_store.exists(path).await.unwrap();
    assert!(exists);

    // Retrieve message
    let retrieved = mail_store.retrieve(path).await.unwrap();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap(), content);

    // Delete message
    mail_store.delete(path).await.unwrap();

    let exists_after = mail_store.exists(path).await.unwrap();
    assert!(!exists_after);

    mail_store.shutdown().await.unwrap();
}

#[async_std::test]
async fn test_index_metadata_operations() {
    let index = InMemoryIndex::new();

    // Create mailbox
    index.create_mailbox("alice", "INBOX").await.unwrap();
    index.create_mailbox("alice", "Sent").await.unwrap();

    // List mailboxes
    let mailboxes = index.list_mailboxes("alice").await.unwrap();
    assert_eq!(mailboxes.len(), 2);

    // Add message
    let message = IndexedMessage {
        id: MessageId::new(),
        uid: 1,
        mailbox: "INBOX".to_string(),
        flags: vec![],
        from: "bob@example.com".to_string(),
        to: "alice@example.com".to_string(),
        subject: "Hello".to_string(),
        body_preview: "Hello there".to_string(),
        internal_date: chrono::Utc::now(),
        size: 100,
    };

    let path = index
        .add_message("alice", "INBOX", message.clone())
        .await
        .unwrap();
    assert!(path.contains("alice/INBOX"));

    // Get message path by ID
    let retrieved_path = index.get_message_path(message.id).await.unwrap();
    assert_eq!(retrieved_path, Some(path.clone()));

    // Get message path by UID
    let uid_path = index
        .get_message_path_by_uid("alice", "INBOX", 1)
        .await
        .unwrap();
    assert_eq!(uid_path, Some(path));

    // Search
    let results = index
        .search("alice", "INBOX", &SearchQuery::From("bob".to_string()))
        .await
        .unwrap();
    assert_eq!(results.len(), 1);

    // Delete mailbox
    index.delete_mailbox("alice", "Sent").await.unwrap();
    let mailboxes_after = index.list_mailboxes("alice").await.unwrap();
    assert_eq!(mailboxes_after.len(), 1);

    index.shutdown().await.unwrap();
}

#[async_std::test]
async fn test_queue_operations() {
    let queue = ChannelQueue::new();

    let task1 = QueueTask::new(
        TaskType::UidCompaction,
        serde_json::json!({"mailbox": "INBOX"}),
    );
    let task2 = QueueTask::new(
        TaskType::IndexUpdate,
        serde_json::json!({"message_id": "12345"}),
    );

    queue.enqueue(task1.clone()).await.unwrap();
    queue.enqueue(task2.clone()).await.unwrap();

    assert_eq!(queue.len().await.unwrap(), 2);

    let dequeued = queue.dequeue().await.unwrap();
    assert!(dequeued.is_some());
    assert_eq!(dequeued.unwrap().id, task1.id);

    assert_eq!(queue.len().await.unwrap(), 1);

    queue.shutdown().await.unwrap();
}

#[async_std::test]
async fn test_userstore_operations() {
    let tmp_dir = TempDir::new().unwrap();
    let db_path = tmp_dir.path().join("users.db");
    let user_store = SQLiteUserStore::new(&db_path).await.unwrap();

    user_store
        .create_user("alice", "password123")
        .await
        .unwrap();
    user_store.create_user("bob", "password456").await.unwrap();

    let user = user_store.get_user("alice").await.unwrap();
    assert!(user.is_some());
    assert_eq!(user.unwrap().username, "alice");

    let valid = user_store
        .verify_password("alice", "password123")
        .await
        .unwrap();
    assert!(valid);

    let invalid = user_store
        .verify_password("alice", "wrongpass")
        .await
        .unwrap();
    assert!(!invalid);

    user_store
        .update_password("alice", "newpass")
        .await
        .unwrap();
    let valid_new = user_store
        .verify_password("alice", "newpass")
        .await
        .unwrap();
    assert!(valid_new);

    let users = user_store.list_users().await.unwrap();
    assert_eq!(users.len(), 2);

    user_store.delete_user("bob").await.unwrap();
    let users_after = user_store.list_users().await.unwrap();
    assert_eq!(users_after.len(), 1);
}

#[async_std::test]
async fn test_authenticator() {
    let tmp_dir = TempDir::new().unwrap();
    let db_path = tmp_dir.path().join("users.db");
    let user_store = Arc::new(SQLiteUserStore::new(&db_path).await.unwrap());
    let authenticator = BasicAuthenticator::new(user_store);

    authenticator
        .user_store
        .create_user("testuser", "testpass")
        .await
        .unwrap();

    let creds = Credentials {
        username: "testuser".to_string(),
        password: "testpass".to_string(),
    };

    let result = authenticator.authenticate(&creds).await.unwrap();
    assert!(result);

    let wrong_creds = Credentials {
        username: "testuser".to_string(),
        password: "wrongpass".to_string(),
    };

    let result = authenticator.authenticate(&wrong_creds).await.unwrap();
    assert!(!result);
}

#[async_std::test]
async fn test_full_integration_workflow() {
    let tmp_dir = TempDir::new().unwrap();
    let mail_dir = tmp_dir.path().join("mail");
    let db_path = tmp_dir.path().join("users.db");

    std::fs::create_dir_all(&mail_dir).unwrap();

    let mail_store = FilesystemMailStore::new(&mail_dir).await.unwrap();
    let index = InMemoryIndex::new();
    let user_store = Arc::new(SQLiteUserStore::new(&db_path).await.unwrap());
    let authenticator = BasicAuthenticator::new(user_store);

    // Create user
    authenticator
        .user_store
        .create_user("alice", "password")
        .await
        .unwrap();

    // Authenticate
    let creds = Credentials {
        username: "alice".to_string(),
        password: "password".to_string(),
    };
    let auth_result = authenticator.authenticate(&creds).await.unwrap();
    assert!(auth_result);

    // Create mailbox
    index.create_mailbox("alice", "INBOX").await.unwrap();

    // Add message via index
    let message = IndexedMessage {
        id: MessageId::new(),
        uid: 1,
        mailbox: "INBOX".to_string(),
        flags: vec![],
        from: "bob@example.com".to_string(),
        to: "alice@example.com".to_string(),
        subject: "Important".to_string(),
        body_preview: "This is important".to_string(),
        internal_date: chrono::Utc::now(),
        size: 100,
    };

    let path = index
        .add_message("alice", "INBOX", message.clone())
        .await
        .unwrap();

    // Store message content in mailstore
    let content = b"From: bob@example.com\r\nTo: alice@example.com\r\nSubject: Important\r\n\r\nThis is important";
    mail_store.store(&path, content).await.unwrap();

    // Search via index
    let search_results = index
        .search(
            "alice",
            "INBOX",
            &SearchQuery::Subject("Important".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(search_results.len(), 1);

    // Retrieve via mailstore using path from index
    let retrieved_path = index.get_message_path(message.id).await.unwrap().unwrap();
    let retrieved_content = mail_store.retrieve(&retrieved_path).await.unwrap();
    assert!(retrieved_content.is_some());
    assert_eq!(retrieved_content.unwrap(), content);

    // Clean up
    mail_store.delete(&path).await.unwrap();
    index.delete_message(message.id).await.unwrap();

    mail_store.shutdown().await.unwrap();
    index.shutdown().await.unwrap();
}
