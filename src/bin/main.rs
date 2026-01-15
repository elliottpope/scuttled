//! Scuttled IMAP server binary

use tokio::fs;
use futures::prelude::*;
use scuttled::authenticator::r#impl::BasicAuthenticator;
use scuttled::index::r#impl::create_inmemory_index;
use scuttled::mailboxes::r#impl::InMemoryMailboxes;
use scuttled::mailstore::r#impl::FilesystemMailStore;
use scuttled::queue::r#impl::ChannelQueue;
use scuttled::server::ImapServer;
use scuttled::userstore::r#impl::SQLiteUserStore;
use scuttled::{Mailboxes, UserStore};
use signal_hook::consts::signal::*;
use signal_hook_tokio::Signals;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let data_dir = PathBuf::from("./data");
    let mail_dir = data_dir.join("mail");
    let db_path = data_dir.join("users.db");

    std::fs::create_dir_all(&data_dir)?;
    std::fs::create_dir_all(&mail_dir)?;

    log::info!("Initializing IMAP server components...");

    let mail_store = FilesystemMailStore::new(&mail_dir).await?;
    let user_store = Arc::new(SQLiteUserStore::new(&db_path).await?);
    let mailboxes = Arc::new(InMemoryMailboxes::new());
    let index = create_inmemory_index(Some(mailboxes.clone()));

    log::info!("Creating default test user...");
    if let Err(e) = user_store.create_user("test", "test").await {
        log::warn!("Failed to create test user (may already exist): {}", e);
    }

    log::info!("Creating default INBOX...");
    if let Err(e) = mailboxes.create_mailbox("test", "INBOX").await {
        log::warn!("Failed to create INBOX (may already exist): {}", e);
    }

    let authenticator = BasicAuthenticator::new(user_store.clone());
    let queue = ChannelQueue::new();

    let cert = fs::read("tls/cert.pem")
        .await
        .expect("failed to read test certificate");
    let key = fs::read("tls/key.pem")
        .await
        .expect("failed to read key file");

    let server = ImapServer::new(
        mail_store,
        index,
        authenticator,
        user_store,
        queue,
        mailboxes,
    )
    .with_tls_pem(&cert, &key)
    .expect("failed to add TLS config to IMAP server");

    let plain_addr = "127.0.0.1:1143";
    let tls_addr = "127.0.0.1:1993";

    log::info!("Starting IMAP servers...");
    log::info!("  Plain IMAP: {}", plain_addr);
    log::info!("  IMAP with TLS: {}", tls_addr);
    log::info!("Default credentials: username=test, password=test");

    // Set up signal handling for graceful shutdown
    let signals = Signals::new([SIGTERM, SIGINT, SIGHUP])?;
    let handle = signals.handle();

    // Clone server for concurrent use
    let server_plain = server.clone();
    let server_tls = server.clone();

    // Spawn plain IMAP server
    let _plain_task = tokio::spawn(async move {
        if let Err(e) = server_plain.listen(plain_addr).await {
            log::error!("Plain IMAP server error: {}", e);
        }
    });

    // Spawn TLS IMAP server
    let _tls_task = tokio::spawn(async move {
        if let Err(e) = server_tls.listen_tls(tls_addr).await {
            log::error!("TLS IMAP server error: {}", e);
        }
    });

    // Wait for shutdown signal
    let mut signals = signals.fuse();
    if let Some(signal) = signals.next().await {
        let signal_name = match signal {
            SIGTERM => "SIGTERM",
            SIGINT => "SIGINT",
            SIGHUP => "SIGHUP",
            _ => "unknown signal",
        };
        log::info!(
            "Received {} signal, initiating graceful shutdown...",
            signal_name
        );
    }

    // Clean up signal handler
    handle.close();

    log::info!("Shutdown complete");
    Ok(())
}
