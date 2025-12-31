//! Maildir format implementation
//!
//! Maildir is a common mail storage format where messages are stored as individual files.
//! The format uses three directories:
//! - `new/`: New messages that haven't been seen
//! - `cur/`: Current messages (already seen)
//! - `tmp/`: Temporary files for message delivery
//!
//! Filenames in Maildir follow the format: `<unique-id>:2,<flags>`
//! where flags can include:
//! - D = Draft
//! - F = Flagged
//! - P = Passed (forwarded)
//! - R = Replied
//! - S = Seen
//! - T = Deleted (marked for deletion)

use crate::mailstore::format::{MailboxFormat, ParsedMessage};
use crate::types::MessageFlag;
use std::path::Path;

/// Maildir format parser
pub struct MaildirFormat;

impl MaildirFormat {
    pub fn new() -> Self {
        Self
    }

    /// Parse Maildir flag characters into MessageFlag objects
    fn parse_flags(flags_str: &str) -> Vec<MessageFlag> {
        let mut flags = Vec::new();

        for ch in flags_str.chars() {
            let flag = match ch {
                'D' => MessageFlag::Draft,
                'F' => MessageFlag::Flagged,
                'R' => MessageFlag::Answered, // R = Replied
                'S' => MessageFlag::Seen,
                'T' => MessageFlag::Deleted, // T = Trashed
                _ => continue,               // Ignore unknown flags
            };
            flags.push(flag);
        }

        flags
    }
}

impl MailboxFormat for MaildirFormat {
    fn parse_message_path(&self, path: &Path, root: &Path) -> Option<ParsedMessage> {
        // Extract relative path from root
        let relative = path.strip_prefix(root).ok()?;
        let components: Vec<_> = relative.components().collect();

        // Need at least username/mailbox/subdir/filename
        if components.len() < 4 {
            return None;
        }

        let username = components[0].as_os_str().to_str()?.to_string();
        let mailbox = components[1].as_os_str().to_str()?.to_string();
        let subdir = components[2].as_os_str().to_str()?;

        // Only process messages in 'new' or 'cur', not 'tmp'
        let is_new = match subdir {
            "new" => true,
            "cur" => false,
            _ => return None, // Ignore tmp and other directories
        };

        // Parse the filename
        let filename = path.file_name()?.to_str()?;

        // Split filename at colon to get unique_id and flags
        let (unique_id, flags) = if let Some(colon_idx) = filename.find(':') {
            let (id, rest) = filename.split_at(colon_idx);
            // Check for ":2," which indicates the start of flags
            if rest.starts_with(":2,") {
                (id.to_string(), Self::parse_flags(&rest[3..]))
            } else {
                (id.to_string(), Vec::new())
            }
        } else {
            // No flags in filename
            (filename.to_string(), Vec::new())
        };

        Some(ParsedMessage {
            unique_id,
            flags,
            is_new,
            username,
            mailbox,
        })
    }

    fn watch_subdirectories(&self) -> Vec<&'static str> {
        vec!["new", "cur"]
    }

    fn flags_to_filename_component(&self, flags: &[MessageFlag]) -> String {
        let mut chars: Vec<char> = Vec::new();

        for flag in flags {
            match flag {
                MessageFlag::Draft => chars.push('D'),
                MessageFlag::Flagged => chars.push('F'),
                MessageFlag::Answered => chars.push('R'),
                MessageFlag::Seen => chars.push('S'),
                MessageFlag::Deleted => chars.push('T'),
                MessageFlag::Recent => {}          // Recent is not stored in Maildir
                MessageFlag::Custom(_) => {}       // Custom flags not supported in standard Maildir
            }
        }

        // Sort flags alphabetically (Maildir convention)
        chars.sort_unstable();

        if chars.is_empty() {
            String::new()
        } else {
            format!(":2,{}", chars.into_iter().collect::<String>())
        }
    }

    fn is_valid_message_file(&self, path: &Path) -> bool {
        // Must be a file (not directory)
        if !path.is_file() {
            return false;
        }

        // Parent must be 'new' or 'cur'
        if let Some(parent) = path.parent() {
            if let Some(parent_name) = parent.file_name().and_then(|n| n.to_str()) {
                return parent_name == "new" || parent_name == "cur";
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_parse_maildir_filename_with_flags() {
        let root = PathBuf::from("/mail");
        let path = PathBuf::from("/mail/user/INBOX/cur/1234567890.M123P456.hostname:2,RS");
        let format = MaildirFormat::new();

        let msg = format.parse_message_path(&path, &root).unwrap();

        assert_eq!(msg.unique_id, "1234567890.M123P456.hostname");
        assert_eq!(msg.username, "user");
        assert_eq!(msg.mailbox, "INBOX");
        assert_eq!(msg.flags.len(), 2);
        assert!(msg.flags.contains(&MessageFlag::Answered));
        assert!(msg.flags.contains(&MessageFlag::Seen));
        assert!(!msg.is_new);
    }

    #[test]
    fn test_parse_maildir_filename_without_flags() {
        let root = PathBuf::from("/mail");
        let path = PathBuf::from("/mail/user/INBOX/new/1234567890.M123P456.hostname");
        let format = MaildirFormat::new();

        let msg = format.parse_message_path(&path, &root).unwrap();

        assert_eq!(msg.unique_id, "1234567890.M123P456.hostname");
        assert_eq!(msg.flags.len(), 0);
        assert!(msg.is_new);
    }

    #[test]
    fn test_parse_maildir_filename_all_flags() {
        let root = PathBuf::from("/mail");
        let path = PathBuf::from("/mail/user/INBOX/cur/123.456:2,DFRST");
        let format = MaildirFormat::new();

        let msg = format.parse_message_path(&path, &root).unwrap();

        assert_eq!(msg.flags.len(), 5);
        assert!(msg.flags.contains(&MessageFlag::Draft));
        assert!(msg.flags.contains(&MessageFlag::Flagged));
        assert!(msg.flags.contains(&MessageFlag::Answered));
        assert!(msg.flags.contains(&MessageFlag::Seen));
        assert!(msg.flags.contains(&MessageFlag::Deleted));
    }

    #[test]
    fn test_flags_to_filename_component() {
        let format = MaildirFormat::new();
        let flags = vec![
            MessageFlag::Seen,
            MessageFlag::Answered,
            MessageFlag::Flagged,
        ];

        let component = format.flags_to_filename_component(&flags);
        assert_eq!(component, ":2,FRS"); // Alphabetically sorted
    }

    #[test]
    fn test_parse_ignores_tmp() {
        let root = PathBuf::from("/mail");
        let path = PathBuf::from("/mail/alice/INBOX/tmp/123.456");
        let format = MaildirFormat::new();

        assert!(format.parse_message_path(&path, &root).is_none());
    }

    #[test]
    fn test_watch_subdirectories() {
        let format = MaildirFormat::new();
        let subdirs = format.watch_subdirectories();

        assert_eq!(subdirs.len(), 2);
        assert!(subdirs.contains(&"new"));
        assert!(subdirs.contains(&"cur"));
    }
}
