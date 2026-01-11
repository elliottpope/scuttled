//! TLS and STARTTLS integration tests

use scuttled::authenticator::r#impl::BasicAuthenticator;
use scuttled::index::r#impl::create_inmemory_index;
use scuttled::mailboxes::r#impl::InMemoryMailboxes;
use scuttled::mailstore::r#impl::FilesystemMailStore;
use scuttled::queue::r#impl::ChannelQueue;
use scuttled::server::ImapServer;
use scuttled::userstore::r#impl::SQLiteUserStore;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_native_tls::TlsConnector;

/// Helper to read a line from the server
async fn read_line<S: AsyncBufReadExt + Unpin>(reader: &mut S) -> String {
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    line.trim_end().to_string()
}

/// Generate self-signed certificate and key using OpenSSL
fn generate_test_certificate() -> (Vec<u8>, Vec<u8>) {
    use std::process::Command;

    let temp_dir = TempDir::new().unwrap();
    let cert_path = temp_dir.path().join("cert.pem");
    let key_path = temp_dir.path().join("key.pem");

    // Generate self-signed certificate
    let output = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-keyout",
            key_path.to_str().unwrap(),
            "-out",
            cert_path.to_str().unwrap(),
            "-days",
            "1",
            "-nodes",
            "-subj",
            "/CN=localhost",
        ])
        .output()
        .expect("Failed to execute openssl. Make sure openssl is installed.");

    if !output.status.success() {
        panic!(
            "OpenSSL failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let cert = std::fs::read(&cert_path).unwrap();
    let key = std::fs::read(&key_path).unwrap();

    (cert, key)
}

#[tokio::test]
async fn test_starttls_capability_advertised() {
    // Set up test server
    let tmp_dir = TempDir::new().unwrap();
    let mail_dir = tmp_dir.path().join("mail");
    let db_path = tmp_dir.path().join("users.db");
    std::fs::create_dir_all(&mail_dir).unwrap();

    let mail_store = FilesystemMailStore::new(&mail_dir).await.unwrap();
    let mailboxes = Arc::new(InMemoryMailboxes::new());
    let index = create_inmemory_index(Some(mailboxes.clone()));
    let user_store = Arc::new(SQLiteUserStore::new(&db_path).await.unwrap());
    let queue = ChannelQueue::new();
    let authenticator = BasicAuthenticator::new(user_store.clone());

    let server = ImapServer::new(
        mail_store,
        index,
        authenticator,
        user_store,
        queue,
        mailboxes,
    );

    // Bind to random port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Spawn server
    tokio::spawn(async move {
        let _ = server.listen_on(listener).await;
    });

    // Give server time to start
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Connect and test CAPABILITY
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let (read_half, mut write_half) = stream.split();
    let mut reader = BufReader::new(read_half);

    // Read greeting
    let greeting = read_line(&mut reader).await;
    assert!(greeting.contains("OK"));
    assert!(greeting.contains("IMAP server ready"));

    // Send CAPABILITY command
    write_half.write_all(b"A001 CAPABILITY\r\n").await.unwrap();
    write_half.flush().await.unwrap();

    let response = read_line(&mut reader).await;
    assert!(response.contains("OK"));
    assert!(response.contains("CAPABILITY"));
    assert!(response.contains("IMAP4rev1"));
    assert!(
        response.contains("STARTTLS"),
        "STARTTLS should be advertised on plain connection"
    );
}

#[tokio::test]
async fn test_starttls_upgrade() {
    // Set up test server with TLS
    let tmp_dir = TempDir::new().unwrap();
    let mail_dir = tmp_dir.path().join("mail");
    let db_path = tmp_dir.path().join("users.db");
    std::fs::create_dir_all(&mail_dir).unwrap();

    let mail_store = FilesystemMailStore::new(&mail_dir).await.unwrap();
    let mailboxes = Arc::new(InMemoryMailboxes::new());
    let index = create_inmemory_index(Some(mailboxes.clone()));
    let user_store = Arc::new(SQLiteUserStore::new(&db_path).await.unwrap());
    let queue = ChannelQueue::new();
    let authenticator = BasicAuthenticator::new(user_store.clone());

    // Generate test certificate
    let (cert_pem, key_pem) = generate_test_certificate();

    let server = ImapServer::new(
        mail_store,
        index,
        authenticator,
        user_store,
        queue,
        mailboxes,
    )
    .with_tls_pem(&cert_pem, &key_pem)
    .unwrap();

    // Bind to random port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Spawn server
    tokio::spawn(async move {
        let _ = server.listen_on(listener).await;
    });

    // Give server time to start
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Connect
    let stream = TcpStream::connect(addr).await.unwrap();
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    // Read greeting
    let greeting = read_line(&mut reader).await;
    assert!(greeting.contains("OK"));

    // Issue STARTTLS
    write_half.write_all(b"A001 STARTTLS\r\n").await.unwrap();
    write_half.flush().await.unwrap();

    let starttls_response = read_line(&mut reader).await;
    assert!(
        starttls_response.contains("A001 OK"),
        "STARTTLS should succeed: {}",
        starttls_response
    );
    assert!(
        starttls_response.contains("Ready for TLS handshake"),
        "Response: {}",
        starttls_response
    );

    // Now upgrade to TLS
    // Reunite the split stream
    let stream = reader.into_inner().reunite(write_half).unwrap();

    let connector = TlsConnector::from(
        native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap(),
    );

    let mut tls_stream = connector
        .connect("localhost", stream)
        .await
        .expect("TLS handshake should succeed");

    // After TLS upgrade, send CAPABILITY again
    tls_stream.write_all(b"A002 CAPABILITY\r\n").await.unwrap();
    tls_stream.flush().await.unwrap();

    // Read response
    let mut tls_reader = BufReader::new(&mut tls_stream);
    let response = read_line(&mut tls_reader).await;

    assert!(
        response.contains("OK"),
        "CAPABILITY after TLS should succeed: {}",
        response
    );
    assert!(
        response.contains("CAPABILITY"),
        "Response should contain CAPABILITY: {}",
        response
    );
    assert!(
        !response.contains("STARTTLS"),
        "STARTTLS should NOT be advertised after TLS upgrade: {}",
        response
    );
}

#[tokio::test]
async fn test_implicit_tls_connection() {
    // Set up test server with implicit TLS
    let tmp_dir = TempDir::new().unwrap();
    let mail_dir = tmp_dir.path().join("mail");
    let db_path = tmp_dir.path().join("users.db");
    std::fs::create_dir_all(&mail_dir).unwrap();

    let mail_store = FilesystemMailStore::new(&mail_dir).await.unwrap();
    let mailboxes = Arc::new(InMemoryMailboxes::new());
    let index = create_inmemory_index(Some(mailboxes.clone()));
    let user_store = Arc::new(SQLiteUserStore::new(&db_path).await.unwrap());
    let queue = ChannelQueue::new();
    let authenticator = BasicAuthenticator::new(user_store.clone());

    // Generate test certificate
    let (cert_pem, key_pem) = generate_test_certificate();

    let server = ImapServer::new(
        mail_store,
        index,
        authenticator,
        user_store,
        queue,
        mailboxes,
    )
    .with_tls_pem(&cert_pem, &key_pem)
    .unwrap();

    // Bind to random port for implicit TLS
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Spawn server with implicit TLS
    tokio::spawn(async move {
        let _ = server.listen_on_tls(listener).await;
    });

    // Give server time to start
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Connect with TLS from the start
    let stream = TcpStream::connect(addr).await.unwrap();

    let connector = TlsConnector::from(
        native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap(),
    );

    let mut tls_stream = connector
        .connect("localhost", stream)
        .await
        .expect("Implicit TLS connection should succeed");

    let mut reader = BufReader::new(&mut tls_stream);

    // Read greeting over TLS
    let greeting = read_line(&mut reader).await;
    assert!(
        greeting.contains("OK"),
        "Should receive greeting over TLS: {}",
        greeting
    );

    // Send CAPABILITY command
    drop(reader); // Drop reader to release the stream
    tls_stream.write_all(b"A001 CAPABILITY\r\n").await.unwrap();
    tls_stream.flush().await.unwrap();

    let mut reader = BufReader::new(&mut tls_stream);
    let response = read_line(&mut reader).await;
    assert!(response.contains("OK"));
    assert!(response.contains("CAPABILITY"));
    assert!(
        !response.contains("STARTTLS"),
        "STARTTLS should NOT be advertised on implicit TLS connection"
    );
}

#[tokio::test]
async fn test_starttls_not_available_without_tls_config() {
    // Set up test server WITHOUT TLS configuration
    let tmp_dir = TempDir::new().unwrap();
    let mail_dir = tmp_dir.path().join("mail");
    let db_path = tmp_dir.path().join("users.db");
    std::fs::create_dir_all(&mail_dir).unwrap();

    let mail_store = FilesystemMailStore::new(&mail_dir).await.unwrap();
    let mailboxes = Arc::new(InMemoryMailboxes::new());
    let index = create_inmemory_index(Some(mailboxes.clone()));
    let user_store = Arc::new(SQLiteUserStore::new(&db_path).await.unwrap());
    let queue = ChannelQueue::new();
    let authenticator = BasicAuthenticator::new(user_store.clone());

    // Create server WITHOUT TLS
    let server = ImapServer::new(
        mail_store,
        index,
        authenticator,
        user_store,
        queue,
        mailboxes,
    );

    // Bind to random port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Spawn server
    tokio::spawn(async move {
        let _ = server.listen_on(listener).await;
    });

    // Give server time to start
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Connect
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let (read_half, mut write_half) = stream.split();
    let mut reader = BufReader::new(read_half);

    // Read greeting
    let greeting = read_line(&mut reader).await;
    assert!(greeting.contains("OK"));

    // Try to issue STARTTLS (should fail)
    write_half.write_all(b"A001 STARTTLS\r\n").await.unwrap();
    write_half.flush().await.unwrap();

    let response = read_line(&mut reader).await;
    assert!(
        response.contains("A001 BAD"),
        "STARTTLS should fail when TLS not configured: {}",
        response
    );
    assert!(
        response.contains("not available"),
        "Response should indicate STARTTLS is not available: {}",
        response
    );
}
