//! Mailbox format trait and implementations
//!
//! This module provides a trait-based abstraction for different mailbox formats
//! (Maildir, mbox, etc.) allowing the system to parse and interpret mail storage
//! in a format-agnostic way.

use crate::types::MessageFlag;
use std::path::Path;

/// Information parsed from a mailbox message file
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMessage {
    /// Unique identifier for this message
    pub unique_id: String,
    /// Flags associated with the message
    pub flags: Vec<MessageFlag>,
    /// Whether this is a new/unread message
    pub is_new: bool,
    /// Username extracted from path
    pub username: String,
    /// Mailbox name extracted from path
    pub mailbox: String,
}

/// Trait for parsing mailbox storage formats
pub trait MailboxFormat: Send + Sync {
    /// Parse a message file path and extract metadata
    ///
    /// Returns None if the path is not a valid message file for this format
    fn parse_message_path(&self, path: &Path, root: &Path) -> Option<ParsedMessage>;

    /// Get the subdirectories that should be watched for this format
    ///
    /// For example, Maildir uses ["new", "cur"], while mbox might return []
    fn watch_subdirectories(&self) -> Vec<&'static str>;

    /// Convert flags to a filename component for this format
    ///
    /// For Maildir, this would be ":2,FLAGS"
    /// For other formats, this might return empty string
    fn flags_to_filename_component(&self, flags: &[MessageFlag]) -> String;

    /// Check if a path represents a valid message file (not a directory or temp file)
    fn is_valid_message_file(&self, path: &Path) -> bool;
}
