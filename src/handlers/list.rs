//! LIST command handler

use crate::command_handler::CommandHandler;
use crate::connection::Connection;
use crate::error::Result;
use crate::mailboxes::MailboxFilter;
use crate::protocol::Response;
use crate::session_context::{SessionContext, SessionState};
use async_trait::async_trait;

/// Convert an IMAP LIST pattern to a MailboxFilter
///
/// Analyzes the pattern and returns the most efficient filter type:
/// - `*` alone -> All
/// - No wildcards -> Exact
/// - `prefix*` -> Prefix
/// - `*suffix` -> Suffix
/// - Complex patterns -> Regex
fn pattern_to_filter(pattern: &str) -> MailboxFilter {
    // Match all
    if pattern == "*" {
        return MailboxFilter::All;
    }

    // No wildcards - exact match
    if !pattern.contains('*') && !pattern.contains('%') {
        return MailboxFilter::Exact(pattern.to_string());
    }

    // Simple prefix pattern: "Test*"
    if pattern.ends_with('*') && !pattern[..pattern.len()-1].contains('*') && !pattern.contains('%') {
        let prefix = &pattern[..pattern.len()-1];
        return MailboxFilter::Prefix(prefix.to_string());
    }

    // Simple suffix pattern: "*Test"
    if pattern.starts_with('*') && !pattern[1..].contains('*') && !pattern.contains('%') {
        let suffix = &pattern[1..];
        return MailboxFilter::Suffix(suffix.to_string());
    }

    // Complex pattern - convert to regex
    let regex_pattern = imap_pattern_to_regex(pattern);
    MailboxFilter::Regex(regex_pattern)
}

/// Convert an IMAP pattern with wildcards (* and %) to a regex pattern
///
/// - `*` matches zero or more characters (including /)
/// - `%` matches zero or more characters but not /
fn imap_pattern_to_regex(pattern: &str) -> String {
    let mut regex = String::from("^");

    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '%' => regex.push_str("[^/]*"),
            // Escape regex special characters
            '.' | '+' | '?' | '^' | '$' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\' => {
                regex.push('\\');
                regex.push(ch);
            }
            _ => regex.push(ch),
        }
    }

    regex.push('$');
    regex
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

        // Convert the IMAP pattern to a filter
        let filter = pattern_to_filter(&effective_pattern);

        // List mailboxes using the filter (backend handles filtering)
        match context.mailboxes.list_mailboxes(username, &filter).await {
            Ok(mailboxes) => {
                // Send individual untagged responses for each mailbox
                for mailbox in &mailboxes {
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
    fn test_pattern_to_filter() {
        // Test match all
        match pattern_to_filter("*") {
            MailboxFilter::All => {},
            _ => panic!("Expected All filter for '*'"),
        }

        // Test exact match
        match pattern_to_filter("INBOX") {
            MailboxFilter::Exact(s) if s == "INBOX" => {},
            _ => panic!("Expected Exact filter for 'INBOX'"),
        }

        // Test prefix match
        match pattern_to_filter("Test*") {
            MailboxFilter::Prefix(s) if s == "Test" => {},
            _ => panic!("Expected Prefix filter for 'Test*'"),
        }

        // Test suffix match
        match pattern_to_filter("*Test") {
            MailboxFilter::Suffix(s) if s == "Test" => {},
            _ => panic!("Expected Suffix filter for '*Test'"),
        }

        // Test complex pattern (regex)
        match pattern_to_filter("Test*Foo") {
            MailboxFilter::Regex(_) => {},
            _ => panic!("Expected Regex filter for 'Test*Foo'"),
        }

        // Test % wildcard (regex)
        match pattern_to_filter("Test%") {
            MailboxFilter::Regex(_) => {},
            _ => panic!("Expected Regex filter for 'Test%'"),
        }
    }

    #[test]
    fn test_imap_pattern_to_regex() {
        // Test * wildcard conversion
        assert_eq!(imap_pattern_to_regex("Test*"), r"^Test.*$");
        assert_eq!(imap_pattern_to_regex("*Test"), r"^.*Test$");

        // Test % wildcard conversion
        assert_eq!(imap_pattern_to_regex("Test%"), r"^Test[^/]*$");
        assert_eq!(imap_pattern_to_regex("%Test"), r"^[^/]*Test$");

        // Test complex patterns
        assert_eq!(imap_pattern_to_regex("Test*Foo%Bar"), r"^Test.*Foo[^/]*Bar$");

        // Test escaping special regex characters
        assert_eq!(imap_pattern_to_regex("Test.Foo"), r"^Test\.Foo$");
        assert_eq!(imap_pattern_to_regex("Test+Foo"), r"^Test\+Foo$");
    }
}
