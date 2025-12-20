//! Scuttled - An async IMAP server implementation in Rust
//!
//! This library provides abstractions and implementations for building
//! an IMAP server using async Rust with async_std.

pub mod error;
pub mod types;
pub mod traits;
pub mod protocol;
pub mod server;
pub mod implementations;

pub use error::{Error, Result};
pub use types::*;
pub use traits::*;

#[cfg(test)]
mod tests;
