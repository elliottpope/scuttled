//! Maildir format parsing and utilities
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

use crate::types::MessageFlag;
use std::path::{Path, PathBuf};

/// Parsed Maildir message information
#[derive(Debug, Clone)]
pub struct MaildirMessage {
    /// The unique identifier (before the colon)
    pub unique_id: String,
    /// The flags extracted from the filename
    pub flags: Vec<MessageFlag>,
    /// Whether this message is in the 'new' or 'cur' directory
    pub is_new: bool,
    /// Full path to the message file
    pub path: PathBuf,
}

/// Parse a Maildir filename to extract flags
///
/// Maildir filenames have the format: `<unique-id>:2,<flags>`
/// For example: `1234567890.M123P456.hostname:2,RS` means Replied and Seen
pub fn parse_maildir_filename(path: &Path) -> Option<MaildirMessage> {
    let filename = path.file_name()?.to_str()?;
    let parent = path.parent()?;
    let parent_name = parent.file_name()?.to_str()?;

    // Determine if message is in 'new' or 'cur' directory
    let is_new = parent_name == "new";

    // Split filename at colon to get unique_id and flags
    let (unique_id, flags_str) = if let Some(colon_idx) = filename.find(':') {
        let (id, rest) = filename.split_at(colon_idx);
        // Check for ":2," which indicates the start of flags
        if rest.starts_with(":2,") {
            (id.to_string(), &rest[3..])
        } else {
            (id.to_string(), "")
        }
    } else {
        // No flags in filename
        (filename.to_string(), "")
    };

    let flags = parse_maildir_flags(flags_str);

    Some(MaildirMessage {
        unique_id,
        flags,
        is_new,
        path: path.to_path_buf(),
    })
}

/// Parse Maildir flag characters into MessageFlag objects
fn parse_maildir_flags(flags_str: &str) -> Vec<MessageFlag> {
    let mut flags = Vec::new();

    for ch in flags_str.chars() {
        let flag = match ch {
            'D' => MessageFlag::Draft,
            'F' => MessageFlag::Flagged,
            'R' => MessageFlag::Answered,  // R = Replied
            'S' => MessageFlag::Seen,
            'T' => MessageFlag::Deleted,   // T = Trashed
            _ => continue,  // Ignore unknown flags
        };
        flags.push(flag);
    }

    // Messages in 'cur' directory without 'S' flag might still be unseen
    // Messages in 'new' directory are always unseen (no 'S' flag)

    flags
}

/// Convert MessageFlag objects back to Maildir flag characters
pub fn flags_to_maildir_string(flags: &[MessageFlag]) -> String {
    let mut chars: Vec<char> = Vec::new();

    for flag in flags {
        match flag {
            MessageFlag::Draft => chars.push('D'),
            MessageFlag::Flagged => chars.push('F'),
            MessageFlag::Answered => chars.push('R'),
            MessageFlag::Seen => chars.push('S'),
            MessageFlag::Deleted => chars.push('T'),
            MessageFlag::Recent => {}, // Recent is not stored in Maildir
            MessageFlag::Custom(_) => {}, // Custom flags not supported in standard Maildir
        }
    }

    // Sort flags alphabetically (Maildir convention)
    chars.sort_unstable();
    chars.into_iter().collect()
}

/// Extract username and mailbox name from a Maildir path
///
/// Expected path format: `<root>/<username>/<mailbox>/[new|cur|tmp]/<filename>`
pub fn parse_maildir_path(path: &Path, root: &Path) -> Option<(String, String, String)> {
    let relative = path.strip_prefix(root).ok()?;
    let components: Vec<_> = relative.components().collect();

    if components.len() < 3 {
        return None;
    }

    let username = components[0].as_os_str().to_str()?.to_string();
    let mailbox = components[1].as_os_str().to_str()?.to_string();

    // The subdirectory should be one of: new, cur, tmp
    let subdir = if components.len() >= 3 {
        components[2].as_os_str().to_str()?.to_string()
    } else {
        return None;
    };

    // Only process messages in 'new' or 'cur', not 'tmp'
    if subdir != "new" && subdir != "cur" {
        return None;
    }

    Some((username, mailbox, subdir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_parse_maildir_filename_with_flags() {
        let path = PathBuf::from("/mail/user/INBOX/cur/1234567890.M123P456.hostname:2,RS");
        let msg = parse_maildir_filename(&path).unwrap();

        assert_eq!(msg.unique_id, "1234567890.M123P456.hostname");
        assert_eq!(msg.flags.len(), 2);
        assert!(msg.flags.contains(&MessageFlag::Answered));
        assert!(msg.flags.contains(&MessageFlag::Seen));
        assert!(!msg.is_new);
    }

    #[test]
    fn test_parse_maildir_filename_without_flags() {
        let path = PathBuf::from("/mail/user/INBOX/new/1234567890.M123P456.hostname");
        let msg = parse_maildir_filename(&path).unwrap();

        assert_eq!(msg.unique_id, "1234567890.M123P456.hostname");
        assert_eq!(msg.flags.len(), 0);
        assert!(msg.is_new);
    }

    #[test]
    fn test_parse_maildir_filename_all_flags() {
        let path = PathBuf::from("/mail/user/INBOX/cur/123.456:2,DFRST");
        let msg = parse_maildir_filename(&path).unwrap();

        assert_eq!(msg.flags.len(), 5);
        assert!(msg.flags.contains(&MessageFlag::Draft));
        assert!(msg.flags.contains(&MessageFlag::Flagged));
        assert!(msg.flags.contains(&MessageFlag::Answered));
        assert!(msg.flags.contains(&MessageFlag::Seen));
        assert!(msg.flags.contains(&MessageFlag::Deleted));
    }

    #[test]
    fn test_flags_to_maildir_string() {
        let flags = vec![
            MessageFlag::Seen,
            MessageFlag::Answered,
            MessageFlag::Flagged,
        ];

        let maildir_str = flags_to_maildir_string(&flags);
        assert_eq!(maildir_str, "FRS");  // Alphabetically sorted
    }

    #[test]
    fn test_parse_maildir_path() {
        let root = PathBuf::from("/mail");
        let path = PathBuf::from("/mail/alice/INBOX/cur/123.456:2,S");

        let (username, mailbox, subdir) = parse_maildir_path(&path, &root).unwrap();
        assert_eq!(username, "alice");
        assert_eq!(mailbox, "INBOX");
        assert_eq!(subdir, "cur");
    }

    #[test]
    fn test_parse_maildir_path_ignores_tmp() {
        let root = PathBuf::from("/mail");
        let path = PathBuf::from("/mail/alice/INBOX/tmp/123.456");

        assert!(parse_maildir_path(&path, &root).is_none());
    }
}
