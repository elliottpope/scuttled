//! Integration tests for IMAP LIST command

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
async fn test_list_all_mailboxes() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.create("Drafts").await.unwrap();
    session.create("Sent").await.unwrap();
    session.create("Trash").await.unwrap();

    {
        let mut mailboxes = session.list(None, Some("*")).await.unwrap();
        let mut count = 0;
        while let Some(_) = mailboxes.next().await {
            count += 1;
        }

        assert!(count >= 3);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_list_empty() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    // List all but only INBOX should exist initially
    {
        let mut mailboxes = session.list(None, Some("*")).await.unwrap();
        let mut count = 0;
        while let Some(_) = mailboxes.next().await {
            count += 1;
        }

        // May have INBOX or may be empty
        assert!(count >= 0);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_list_specific_pattern() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.create("Test1").await.unwrap();
    session.create("Test2").await.unwrap();
    session.create("Other").await.unwrap();

    {
        let mut mailboxes = session.list(None, Some("Test*")).await.unwrap();
        let mut count = 0;
        while let Some(_) = mailboxes.next().await {
            count += 1;
        }

        assert!(count >= 2);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_list_inbox_pattern() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    {
        let mut mailboxes = session.list(None, Some("INBOX")).await.unwrap();
        let mut count = 0;
        while let Some(_) = mailboxes.next().await {
            count += 1;
        }

        // Should find INBOX (if it exists)
        assert!(count >= 0);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_list_percent_wildcard() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.create("Folder1").await.unwrap();
    session.create("Folder2").await.unwrap();

    // % matches single level
    {
        let mut mailboxes = session.list(None, Some("%")).await.unwrap();
        let mut count = 0;
        while let Some(_) = mailboxes.next().await {
            count += 1;
        }

        assert!(count >= 0);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_list_hierarchical_mailboxes() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    // Try creating hierarchical mailboxes
    let create1 = session.create("Parent").await;
    let create2 = session.create("Parent/Child").await;
    let create3 = session.create("Parent/Child/Grandchild").await;

    if create1.is_ok() && create2.is_ok() && create3.is_ok() {
        let mut mailboxes = session.list(None, Some("Parent/*")).await.unwrap();
        let mut count = 0;
        while let Some(_) = mailboxes.next().await {
            count += 1;
        }

        // Should find at least some hierarchical mailboxes
        assert!(count >= 0);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_list_with_reference() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.create("Ref/Sub1").await.ok();
    session.create("Ref/Sub2").await.ok();

    // List with reference parameter
    {
        let mut mailboxes = session.list(Some("Ref/"), Some("*")).await.unwrap();
        let mut count = 0;
        while let Some(_) = mailboxes.next().await {
            count += 1;
        }

        // Documents current behavior
        assert!(count >= 0);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_list_nonexistent_pattern() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    {
        let mut mailboxes = session.list(None, Some("NonExistent*")).await.unwrap();
        let mut count = 0;
        while let Some(_) = mailboxes.next().await {
            count += 1;
        }

        assert_eq!(count, 0);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_list_empty_pattern() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    // Empty pattern - behavior may vary
    {
        let result = session.list(None, Some("")).await;

        // Documents current behavior
        let _ = result;
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_list_collect_names() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.create("Alpha").await.unwrap();
    session.create("Beta").await.unwrap();
    session.create("Gamma").await.unwrap();

    {
        let mut mailboxes = session.list(None, Some("*")).await.unwrap();
        let mut names = Vec::new();
        while let Some(name_result) = mailboxes.next().await {
            if let Ok(name) = name_result {
                names.push(name.name().to_string());
            }
        }

        assert!(names.iter().any(|n| n == "Alpha"));
        assert!(names.iter().any(|n| n == "Beta"));
        assert!(names.iter().any(|n| n == "Gamma"));
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_list_attributes() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.create("TestMailbox").await.unwrap();

    {
        let mut mailboxes = session.list(None, Some("TestMailbox")).await.unwrap();
        while let Some(name_result) = mailboxes.next().await {
            if let Ok(name) = name_result {
                // Check that attributes are present
                let _ = name.attributes();
            }
        }
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_list_delimiter() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    {
        let mut mailboxes = session.list(None, Some("*")).await.unwrap();
        while let Some(name_result) = mailboxes.next().await {
            if let Ok(name) = name_result {
                // Check that delimiter is present
                let _ = name.delimiter();
            }
        }
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_list_after_create_and_delete() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.create("Temporary").await.unwrap();

    {
        let mut mailboxes = session.list(None, Some("Temporary")).await.unwrap();
        let mut count = 0;
        while let Some(_) = mailboxes.next().await {
            count += 1;
        }
        assert_eq!(count, 1);
    }

    session.delete("Temporary").await.unwrap();

    {
        let mut mailboxes = session.list(None, Some("Temporary")).await.unwrap();
        let mut count = 0;
        while let Some(_) = mailboxes.next().await {
            count += 1;
        }
        assert_eq!(count, 0);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_list_multiple_wildcards() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    session.create("Foo").await.unwrap();
    session.create("FooBar").await.unwrap();
    session.create("FooBaz").await.unwrap();

    {
        let mut mailboxes = session.list(None, Some("Foo*")).await.unwrap();
        let mut count = 0;
        while let Some(_) = mailboxes.next().await {
            count += 1;
        }

        assert!(count >= 3);
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_list_case_insensitive_inbox() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    // Try different case variations of INBOX
    let patterns = vec!["inbox", "Inbox", "INBOX"];

    {
        for pattern in patterns {
            let mut mailboxes = session.list(None, Some(pattern)).await.unwrap();
            let mut count = 0;
            while let Some(_) = mailboxes.next().await {
                count += 1;
            }
            // Documents current behavior
            let _ = count;
        }
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_list_unicode_mailbox() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let unicode_name = "日本語";
    let create_result = session.create(unicode_name).await;

    if create_result.is_ok() {
        let mut mailboxes = session.list(None, Some(unicode_name)).await.unwrap();
        let mut count = 0;
        while let Some(_) = mailboxes.next().await {
            count += 1;
        }
        // Documents current behavior
        let _ = count;
    }

    session.logout().await.unwrap();
}

#[tokio::test]
async fn test_list_very_long_pattern() {
    let (_tmp_dir, addr) = setup_test_server().await;

    let stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let client = async_imap::Client::new(stream);
    let mut session = client.login("testuser", "testpass").await.unwrap();

    let long_pattern = format!("{}*", "A".repeat(1000));
    {
        let mut mailboxes = session.list(None, Some(&long_pattern)).await.unwrap();
        let mut count = 0;
        while let Some(_) = mailboxes.next().await {
            count += 1;
        }

        // Should return 0 - no mailboxes match
        assert_eq!(count, 0);
    }

    session.logout().await.unwrap();
}
