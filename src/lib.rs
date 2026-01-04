//! Scuttled - An async IMAP server implementation in Rust
//!
//! This library provides abstractions and implementations for building
//! an IMAP server using async Rust with async_std.

pub mod error;
pub mod events;
pub mod types;
pub mod protocol;
pub mod connection;
pub mod server;
pub mod mailstore;
pub mod storage;
pub mod index;
pub mod searcher;
pub mod queue;
pub mod authenticator;
pub mod userstore;
pub mod command_handler;
pub mod session_context;
pub mod mailboxes;
pub mod session;

pub use error::{Error, Result};
pub use types::*;
pub use mailstore::MailStore;
pub use storage::Storage;
pub use index::Index;
pub use searcher::Searcher;
pub use queue::Queue;
pub use authenticator::Authenticator;
pub use userstore::UserStore;
pub use command_handler::CommandHandler;
pub use session_context::{SessionContext, SessionState};
pub use mailboxes::{Mailboxes, MailboxInfo};

#[cfg(test)]
mod tests;
