//! Filesystem watcher implementations for mail stores
//!
//! This module provides functionality to watch mail directories for changes
//! and propagate them to the Index.

pub mod maildir;
pub mod filesystem;

pub use filesystem::FilesystemWatcher;
pub use maildir::{MaildirMessage, parse_maildir_filename};
