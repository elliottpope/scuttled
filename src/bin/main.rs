//! Scuttled IMAP server binary

use scuttled::implementations::*;
use scuttled::server::ImapServer;
use scuttled::{MailStore, UserStore};
use std::path::PathBuf;

#[async_std::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let data_dir = PathBuf::from("./data");
    let mail_dir = data_dir.join("mail");
    let index_dir = data_dir.join("index");
    let db_path = data_dir.join("users.db");

    std::fs::create_dir_all(&data_dir)?;
    std::fs::create_dir_all(&mail_dir)?;
    std::fs::create_dir_all(&index_dir)?;

    log::info!("Initializing IMAP server components...");

    let mail_store = FilesystemMailStore::new(&mail_dir).await?;
    let index = DefaultIndex::new(&index_dir)?;
    let user_store1 = SQLiteUserStore::new(&db_path).await?;
    let user_store2 = SQLiteUserStore::new(&db_path).await?;

    log::info!("Creating default test user...");
    if let Err(e) = user_store1.create_user("test", "test").await {
        log::warn!("Failed to create test user (may already exist): {}", e);
    }

    log::info!("Creating default INBOX...");
    if let Err(e) = mail_store.create_mailbox("INBOX").await {
        log::warn!("Failed to create INBOX (may already exist): {}", e);
    }

    let authenticator = BasicAuthenticator::new(user_store1);
    let queue = InMemoryQueue::new();

    let server = ImapServer::new(mail_store, index, authenticator, user_store2, queue);

    let addr = "127.0.0.1:1143";
    log::info!("Starting IMAP server on {}...", addr);
    log::info!("Default credentials: username=test, password=test");

    server.listen(addr).await?;

    Ok(())
}
