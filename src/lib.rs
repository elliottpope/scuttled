//! Scuttled - An async IMAP server implementation in Rust
//!
//! This library provides abstractions and implementations for building
//! an IMAP server using async Rust with async_std.

pub mod error;
pub mod types;
pub mod protocol;
pub mod server;
pub mod mailstore;
pub mod index;
pub mod queue;
pub mod authenticator;
pub mod userstore;

pub use error::{Error, Result};
pub use types::*;
pub use mailstore::MailStore;
pub use index::Index;
pub use queue::Queue;
pub use authenticator::Authenticator;
pub use userstore::UserStore;

#[cfg(test)]
mod tests;
