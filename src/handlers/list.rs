//! LIST command handler

use crate::command_handler::CommandHandler;
use crate::connection::Connection;
use crate::error::Result;
use crate::protocol::Response;
use crate::session_context::{SessionContext, SessionState};
use async_trait::async_trait;

/// Match a mailbox name against an IMAP LIST pattern
///
/// Supports:
/// - `*` wildcard: matches zero or more characters at any level
/// - `%` wildcard: matches zero or more characters at current level only (doesn't match hierarchy delimiter)
/// - Exact matches (no wildcards)
/// - INBOX is case-insensitive per IMAP RFC 3501
fn matches_pattern(mailbox_name: &str, pattern: &str) -> bool {
    // Handle wildcard-only patterns
    if pattern == "*" {
        return true;
    }
    if pattern == "%" {
        // % matches anything at the current level (no hierarchy delimiter)
        return !mailbox_name.contains('/');
    }

    // If no wildcards, do exact match
    if !pattern.contains('*') && !pattern.contains('%') {
        // INBOX is case-insensitive
        if mailbox_name.eq_ignore_ascii_case("INBOX") && pattern.eq_ignore_ascii_case("INBOX") {
            return true;
        }
        return mailbox_name == pattern;
    }

    // Handle simple prefix patterns like "Test*"
    if pattern.ends_with('*') && !pattern[..pattern.len()-1].contains('*') && !pattern.contains('%') {
        let prefix = &pattern[..pattern.len()-1];
        return mailbox_name.starts_with(prefix);
    }

    // Handle simple suffix patterns like "*Test"
    if pattern.starts_with('*') && !pattern[1..].contains('*') && !pattern.contains('%') {
        let suffix = &pattern[1..];
        return mailbox_name.ends_with(suffix);
    }

    // For more complex patterns, use character-by-character matching
    wildcard_match(mailbox_name, pattern)
}

/// Perform wildcard matching with * and % support
fn wildcard_match(text: &str, pattern: &str) -> bool {
    let text_chars: Vec<char> = text.chars().collect();
    let pattern_chars: Vec<char> = pattern.chars().collect();

    wildcard_match_impl(&text_chars, &pattern_chars, 0, 0)
}

/// Recursive wildcard matching implementation
fn wildcard_match_impl(text: &[char], pattern: &[char], t_idx: usize, p_idx: usize) -> bool {
    // Base cases
    if p_idx >= pattern.len() {
        return t_idx >= text.len();
    }

    if t_idx >= text.len() {
        // Only match if remaining pattern is all wildcards
        return pattern[p_idx..].iter().all(|&c| c == '*');
    }

    match pattern[p_idx] {
        '*' => {
            // * matches zero or more characters (including /)
            // Try matching zero characters (skip the *)
            if wildcard_match_impl(text, pattern, t_idx, p_idx + 1) {
                return true;
            }
            // Try matching one or more characters
            wildcard_match_impl(text, pattern, t_idx + 1, p_idx)
        }
        '%' => {
            // % matches zero or more characters but not /
            // Try matching zero characters (skip the %)
            if wildcard_match_impl(text, pattern, t_idx, p_idx + 1) {
                return true;
            }
            // Try matching one more character if it's not /
            if text[t_idx] != '/' {
                wildcard_match_impl(text, pattern, t_idx + 1, p_idx)
            } else {
                false
            }
        }
        c => {
            // Literal character match
            if text[t_idx] == c {
                wildcard_match_impl(text, pattern, t_idx + 1, p_idx + 1)
            } else {
                false
            }
        }
    }
}

/// Handler for the LIST command
///
/// Lists mailboxes matching a pattern.
pub struct ListHandler;

impl ListHandler {
    pub fn new() -> Self {
        Self
    }

    /// Parse LIST command arguments
    ///
    /// Format: LIST reference mailbox-pattern
    /// Also accepts: LIST mailbox-pattern (assumes empty reference)
    fn parse_args(args: &str) -> Option<(String, String)> {
        let parts: Vec<&str> = args.split_whitespace().collect();
        match parts.len() {
            1 => {
                // Just pattern, assume empty reference
                Some(("".to_string(), parts[0].to_string()))
            }
            n if n >= 2 => {
                // Reference and pattern
                Some((parts[0].to_string(), parts[1].to_string()))
            }
            _ => None,
        }
    }
}

impl Default for ListHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandHandler for ListHandler {
    fn command_name(&self) -> &str {
        "LIST"
    }

    async fn handle(
        &self,
        tag: &str,
        args: &str,
        connection: &Connection,
        context: &SessionContext,
        current_state: &SessionState,
    ) -> Result<Option<SessionState>> {
        // Parse arguments
        let (reference, pattern) = match Self::parse_args(args) {
            Some((r, p)) => (r, p),
            None => {
                let response = Response::Bad {
                    tag: Some(tag.to_string()),
                    message: "Invalid LIST syntax".to_string(),
                };
                connection.write_response(&response).await?;
                return Ok(None);
            }
        };

        // Get current username from session state
        let username = match current_state {
            SessionState::Authenticated { username } => username,
            SessionState::Selected { username, .. } => username,
            _ => {
                let response = Response::No {
                    tag: Some(tag.to_string()),
                    message: "Not authenticated".to_string(),
                };
                connection.write_response(&response).await?;
                return Ok(None);
            }
        };

        // Combine reference and pattern according to IMAP spec
        // Remove quotes from both reference and pattern
        let clean_ref = reference.trim_matches('"');
        let clean_pattern = pattern.trim_matches('"');

        // If reference is not empty, prepend it to pattern
        let effective_pattern = if !clean_ref.is_empty() {
            // Simply concatenate reference and pattern
            // The reference usually ends with "/" so "Ref/" + "*" = "Ref/*"
            format!("{}{}", clean_ref, clean_pattern)
        } else {
            clean_pattern.to_string()
        };

        // List mailboxes
        match context.mailboxes.list_mailboxes(username).await {
            Ok(mailboxes) => {
                // Filter by pattern using wildcard matching
                let filtered: Vec<_> = mailboxes
                    .into_iter()
                    .filter(|m| matches_pattern(&m.id.name, &effective_pattern))
                    .collect();

                // Send individual untagged responses for each mailbox
                for mailbox in &filtered {
                    let response = Response::Untagged {
                        message: format!("LIST () \"/\" \"{}\"", mailbox.id.name),
                    };
                    connection.write_response(&response).await?;
                }

                // Send final tagged OK response
                let response = Response::Ok {
                    tag: Some(tag.to_string()),
                    message: "LIST completed".to_string(),
                };
                connection.write_response(&response).await?;
                Ok(None)
            }
            Err(e) => {
                let response = Response::No {
                    tag: Some(tag.to_string()),
                    message: format!("LIST failed: {}", e),
                };
                connection.write_response(&response).await?;
                Ok(None)
            }
        }
    }

    fn requires_auth(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_args() {
        assert_eq!(
            ListHandler::parse_args("\"\" *"),
            Some(("\"\"".to_string(), "*".to_string()))
        );

        assert_eq!(
            ListHandler::parse_args("\"\" INBOX"),
            Some(("\"\"".to_string(), "INBOX".to_string()))
        );

        // Single argument - pattern only, empty reference
        assert_eq!(
            ListHandler::parse_args("*"),
            Some(("".to_string(), "*".to_string()))
        );

        assert_eq!(
            ListHandler::parse_args("INBOX"),
            Some(("".to_string(), "INBOX".to_string()))
        );

        // Empty args is invalid
        assert_eq!(ListHandler::parse_args(""), None);
    }

    #[tokio::test]
    async fn test_list_handler() {
        let handler = ListHandler::new();
        assert_eq!(handler.command_name(), "LIST");
        assert!(handler.requires_auth());
    }

    #[test]
    fn test_matches_pattern() {
        // Test exact match
        assert!(matches_pattern("INBOX", "INBOX"));
        assert!(!matches_pattern("INBOX", "Drafts"));

        // Test INBOX case-insensitivity
        assert!(matches_pattern("INBOX", "inbox"));
        assert!(matches_pattern("INBOX", "Inbox"));
        assert!(matches_pattern("INBOX", "INBOX"));
        assert!(matches_pattern("INBOX", "InBoX"));

        // Test * wildcard
        assert!(matches_pattern("Drafts", "*"));
        assert!(matches_pattern("Sent", "*"));
        assert!(matches_pattern("any/thing", "*"));

        // Test prefix matching
        assert!(matches_pattern("Test1", "Test*"));
        assert!(matches_pattern("Test2", "Test*"));
        assert!(matches_pattern("TestABC", "Test*"));
        assert!(!matches_pattern("Other", "Test*"));

        // Test suffix matching
        assert!(matches_pattern("Test", "*est"));
        assert!(matches_pattern("BestTest", "*Test"));
        assert!(!matches_pattern("Testing", "*Test"));

        // Test % wildcard
        assert!(matches_pattern("Test", "%"));
        assert!(matches_pattern("Drafts", "%"));
        assert!(!matches_pattern("Folder/Sub", "%"));
    }
}
