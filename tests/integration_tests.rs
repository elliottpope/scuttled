//! Integration tests for the IMAP server

use async_std::io::{ReadExt, WriteExt};
use async_std::net::TcpStream;
use async_std::prelude::*;
use async_std::task;
use chrono::Utc;
use scuttled::implementations::*;
use scuttled::server::ImapServer;
use scuttled::types::*;
use scuttled::{Authenticator, Index, MailStore, Queue, UserStore};
use tempfile::TempDir;

async fn setup_test_server() -> (TempDir, String) {
    let tmp_dir = TempDir::new().unwrap();
    let mail_dir = tmp_dir.path().join("mail");
    let index_dir = tmp_dir.path().join("index");
    let db_path = tmp_dir.path().join("users.db");

    let mail_store = FilesystemMailStore::new(&mail_dir).await.unwrap();
    let index = DefaultIndex::new(&index_dir).unwrap();
    let user_store1 = SQLiteUserStore::new(&db_path).await.unwrap();
    let user_store2 = SQLiteUserStore::new(&db_path).await.unwrap();

    user_store1.create_user("testuser", "testpass").await.unwrap();
    mail_store.create_mailbox("INBOX").await.unwrap();

    let authenticator = BasicAuthenticator::new(user_store1);
    let queue = InMemoryQueue::new();

    let server = ImapServer::new(mail_store, index, authenticator, user_store2, queue);

    let addr = "127.0.0.1:0".to_string();

    let listener = async_std::net::TcpListener::bind(&addr).await.unwrap();
    let actual_addr = listener.local_addr().unwrap();

    task::spawn(async move {
        let mut incoming = listener.incoming();
        if let Some(Ok(stream)) = incoming.next().await {
            let _ = handle_test_connection(stream).await;
        }
    });

    (tmp_dir, actual_addr.to_string())
}

async fn handle_test_connection(mut stream: TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    stream.write_all(b"* OK IMAP4rev1 Service Ready\r\n").await?;

    let mut buffer = [0u8; 1024];
    while let Ok(n) = stream.read(&mut buffer).await {
        if n == 0 {
            break;
        }
        let request = String::from_utf8_lossy(&buffer[..n]);

        if request.contains("CAPABILITY") {
            stream
                .write_all(b"* CAPABILITY IMAP4rev1 AUTH=PLAIN\r\n")
                .await?;
            stream.write_all(b"A001 OK CAPABILITY completed\r\n").await?;
        } else if request.contains("LOGIN") {
            stream.write_all(b"A002 OK LOGIN completed\r\n").await?;
        } else if request.contains("LOGOUT") {
            stream
                .write_all(b"* BYE IMAP4rev1 Server logging out\r\n")
                .await?;
            break;
        }
    }

    Ok(())
}

#[async_std::test]
async fn test_mailstore_integration() {
    let tmp_dir = TempDir::new().unwrap();
    let mail_store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

    mail_store.create_mailbox("INBOX").await.unwrap();
    mail_store.create_mailbox("Sent").await.unwrap();

    let mailboxes = mail_store.list_mailboxes("testuser").await.unwrap();
    assert_eq!(mailboxes.len(), 2);

    let uid = mail_store.get_next_uid("INBOX").await.unwrap();

    let message = Message {
        id: MessageId::new(),
        uid,
        mailbox: "INBOX".to_string(),
        flags: vec![],
        internal_date: Utc::now(),
        size: 100,
        raw_content: b"From: sender@example.com\nTo: recipient@example.com\nSubject: Test\n\nBody"
            .to_vec(),
    };

    mail_store.store_message("INBOX", &message).await.unwrap();

    let messages = mail_store.list_messages("INBOX").await.unwrap();
    assert_eq!(messages.len(), 1);

    let retrieved = mail_store.get_message(message.id).await.unwrap();
    assert!(retrieved.is_some());
}

#[async_std::test]
async fn test_index_integration() {
    let tmp_dir = TempDir::new().unwrap();
    let index_dir = tmp_dir.path().join("index");
    let index = DefaultIndex::new(&index_dir).unwrap();

    let message1 = Message {
        id: MessageId::new(),
        uid: 1,
        mailbox: "INBOX".to_string(),
        flags: vec![],
        internal_date: Utc::now(),
        size: 100,
        raw_content: b"From: alice@example.com\nTo: bob@example.com\nSubject: Hello\n\nHello Bob!"
            .to_vec(),
    };

    let message2 = Message {
        id: MessageId::new(),
        uid: 2,
        mailbox: "INBOX".to_string(),
        flags: vec![MessageFlag::Seen],
        internal_date: Utc::now(),
        size: 100,
        raw_content: b"From: charlie@example.com\nTo: bob@example.com\nSubject: Meeting\n\nLet's meet"
            .to_vec(),
    };

    index.index_message(&message1).await.unwrap();
    index.index_message(&message2).await.unwrap();

    let results = index
        .search("INBOX", &SearchQuery::From("alice".to_string()))
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], message1.id);

    let results = index
        .search("INBOX", &SearchQuery::Subject("Meeting".to_string()))
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], message2.id);

    let results = index
        .search("INBOX", &SearchQuery::Text("Bob".to_string()))
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
}

#[async_std::test]
async fn test_userstore_integration() {
    let tmp_dir = TempDir::new().unwrap();
    let db_path = tmp_dir.path().join("users.db");
    let user_store = SQLiteUserStore::new(&db_path).await.unwrap();

    user_store.create_user("user1", "password1").await.unwrap();
    user_store.create_user("user2", "password2").await.unwrap();

    let user = user_store.get_user("user1").await.unwrap();
    assert!(user.is_some());
    assert_eq!(user.unwrap().username, "user1");

    let valid = user_store.verify_password("user1", "password1").await.unwrap();
    assert!(valid);

    let invalid = user_store.verify_password("user1", "wrongpass").await.unwrap();
    assert!(!invalid);

    user_store.update_password("user1", "newpass").await.unwrap();

    let valid_new = user_store.verify_password("user1", "newpass").await.unwrap();
    assert!(valid_new);

    let users = user_store.list_users().await.unwrap();
    assert_eq!(users.len(), 2);

    user_store.delete_user("user2").await.unwrap();
    let users = user_store.list_users().await.unwrap();
    assert_eq!(users.len(), 1);
}

#[async_std::test]
async fn test_authenticator_integration() {
    let tmp_dir = TempDir::new().unwrap();
    let db_path = tmp_dir.path().join("users.db");
    let user_store = SQLiteUserStore::new(&db_path).await.unwrap();
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
async fn test_queue_integration() {
    let queue = InMemoryQueue::new();

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

    let peeked = queue.peek().await.unwrap();
    assert!(peeked.is_some());
    assert_eq!(peeked.unwrap().id, task1.id);

    let dequeued = queue.dequeue().await.unwrap();
    assert!(dequeued.is_some());
    assert_eq!(dequeued.unwrap().id, task1.id);

    assert_eq!(queue.len().await.unwrap(), 1);

    queue.clear().await.unwrap();
    assert!(queue.is_empty().await.unwrap());
}

#[async_std::test]
async fn test_full_workflow() {
    let tmp_dir = TempDir::new().unwrap();
    let mail_dir = tmp_dir.path().join("mail");
    let index_dir = tmp_dir.path().join("index");
    let db_path = tmp_dir.path().join("users.db");

    let mail_store = FilesystemMailStore::new(&mail_dir).await.unwrap();
    let index = DefaultIndex::new(&index_dir).unwrap();
    let user_store = SQLiteUserStore::new(&db_path).await.unwrap();
    let authenticator = BasicAuthenticator::new(user_store);
    let queue = InMemoryQueue::new();

    authenticator
        .user_store
        .create_user("alice", "password")
        .await
        .unwrap();

    mail_store.create_mailbox("INBOX").await.unwrap();

    let uid = mail_store.get_next_uid("INBOX").await.unwrap();

    let message = Message {
        id: MessageId::new(),
        uid,
        mailbox: "INBOX".to_string(),
        flags: vec![],
        internal_date: Utc::now(),
        size: 100,
        raw_content: b"From: bob@example.com\nTo: alice@example.com\nSubject: Important\n\nThis is important"
            .to_vec(),
    };

    mail_store.store_message("INBOX", &message).await.unwrap();

    index.index_message(&message).await.unwrap();

    let search_results = index
        .search("INBOX", &SearchQuery::Subject("Important".to_string()))
        .await
        .unwrap();
    assert_eq!(search_results.len(), 1);
    assert_eq!(search_results[0], message.id);

    let task = QueueTask::new(
        TaskType::UidCompaction,
        serde_json::json!({"mailbox": "INBOX"}),
    );
    queue.enqueue(task).await.unwrap();

    let creds = Credentials {
        username: "alice".to_string(),
        password: "password".to_string(),
    };
    let auth_result = authenticator.authenticate(&creds).await.unwrap();
    assert!(auth_result);

    mail_store
        .update_flags(message.id, vec![MessageFlag::Seen])
        .await
        .unwrap();

    let retrieved = mail_store.get_message(message.id).await.unwrap().unwrap();
    assert_eq!(retrieved.flags.len(), 1);
    assert_eq!(retrieved.flags[0], MessageFlag::Seen);

    mail_store.delete_message(message.id).await.unwrap();

    index.remove_message(message.id).await.unwrap();

    let messages = mail_store.list_messages("INBOX").await.unwrap();
    assert_eq!(messages.len(), 0);
}

#[async_std::test]
async fn test_concurrent_operations() {
    let tmp_dir = TempDir::new().unwrap();
    let mail_store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

    mail_store.create_mailbox("INBOX").await.unwrap();

    let mut handles = vec![];

    for i in 0..10 {
        let store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();
        let handle = task::spawn(async move {
            let uid = store.get_next_uid("INBOX").await.unwrap();
            let message = Message {
                id: MessageId::new(),
                uid,
                mailbox: "INBOX".to_string(),
                flags: vec![],
                internal_date: Utc::now(),
                size: 100,
                raw_content: format!("Message {}", i).into_bytes(),
            };
            store.store_message("INBOX", &message).await.unwrap();
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await;
    }

    let messages = mail_store.list_messages("INBOX").await.unwrap();
    assert_eq!(messages.len(), 10);
}

#[async_std::test]
async fn test_mailbox_operations() {
    let tmp_dir = TempDir::new().unwrap();
    let mail_store = FilesystemMailStore::new(tmp_dir.path()).await.unwrap();

    mail_store.create_mailbox("Drafts").await.unwrap();
    mail_store.create_mailbox("Sent").await.unwrap();
    mail_store.create_mailbox("Trash").await.unwrap();

    let mailboxes = mail_store.list_mailboxes("testuser").await.unwrap();
    assert_eq!(mailboxes.len(), 3);

    mail_store.rename_mailbox("Drafts", "MyDrafts").await.unwrap();

    let mailbox = mail_store.get_mailbox("MyDrafts").await.unwrap();
    assert!(mailbox.is_some());
    assert_eq!(mailbox.unwrap().name, "MyDrafts");

    let old_mailbox = mail_store.get_mailbox("Drafts").await.unwrap();
    assert!(old_mailbox.is_none());

    mail_store.delete_mailbox("Trash").await.unwrap();

    let mailboxes = mail_store.list_mailboxes("testuser").await.unwrap();
    assert_eq!(mailboxes.len(), 2);
}
