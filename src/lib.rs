//! Scuttled - An async IMAP server implementation in Rust
//!
//! This library provides abstractions and implementations for building
//! an IMAP server using async Rust with async_std.

pub mod authenticator;
pub mod command_handler;
pub mod command_handlers;
pub mod connection;
pub mod error;
pub mod events;
pub mod handlers;
pub mod index;
pub mod mailbox;
pub mod mailboxes;
pub mod mailstore;
pub mod protocol;
pub mod queue;
pub mod searcher;
pub mod server;
pub mod session;
pub mod session_context;
pub mod storage;
pub mod types;
pub mod userstore;

pub use authenticator::Authenticator;
pub use command_handler::CommandHandler;
pub use command_handlers::CommandHandlers;
pub use error::{Error, Result};
pub use index::Index;
pub use mailbox::{Mailbox, MailboxNotification, MailboxState};
pub use mailboxes::{MailboxInfo, Mailboxes};
pub use mailstore::MailStore;
pub use queue::Queue;
pub use searcher::Searcher;
pub use session_context::{SessionContext, SessionState};
pub use storage::{run_storage_writer_loop, FilesystemStore, Storage, StoreMail};
pub use types::*;
pub use userstore::UserStore;

#[cfg(test)]
mod tests;
