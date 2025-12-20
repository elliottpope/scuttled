//! IMAP protocol handling

use crate::error::{Error, Result};
use std::fmt;

/// IMAP command parsed from client input
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Capability,
    Noop,
    Logout,
    Login { username: String, password: String },
    Select { mailbox: String },
    Examine { mailbox: String },
    Create { mailbox: String },
    Delete { mailbox: String },
    Rename { from: String, to: String },
    List { reference: String, pattern: String },
    Lsub { reference: String, pattern: String },
    Status { mailbox: String, items: Vec<StatusItem> },
    Append { mailbox: String, flags: Vec<String>, date: Option<String>, message: Vec<u8> },
    Check,
    Close,
    Expunge,
    Search { query: String },
    Fetch { sequence: String, items: Vec<FetchItem> },
    Store { sequence: String, flags: Vec<String>, add: bool },
    Copy { sequence: String, mailbox: String },
    Uid { command: Box<Command> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum StatusItem {
    Messages,
    Recent,
    Uidnext,
    Uidvalidity,
    Unseen,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FetchItem {
    Uid,
    Flags,
    InternalDate,
    Rfc822Size,
    Envelope,
    Body,
    BodyStructure,
    BodySection { section: String },
    Fast,
    All,
    Full,
}

/// IMAP response to send to client
#[derive(Debug, Clone)]
pub enum Response {
    Ok { tag: Option<String>, message: String },
    No { tag: Option<String>, message: String },
    Bad { tag: Option<String>, message: String },
    Bye { message: String },
    Untagged { message: String },
    Continuation { message: String },
}

impl fmt::Display for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Response::Ok { tag: Some(tag), message } => write!(f, "{} OK {}\r\n", tag, message),
            Response::Ok { tag: None, message } => write!(f, "* OK {}\r\n", message),
            Response::No { tag: Some(tag), message } => write!(f, "{} NO {}\r\n", tag, message),
            Response::No { tag: None, message } => write!(f, "* NO {}\r\n", message),
            Response::Bad { tag: Some(tag), message } => write!(f, "{} BAD {}\r\n", tag, message),
            Response::Bad { tag: None, message } => write!(f, "* BAD {}\r\n", message),
            Response::Bye { message } => write!(f, "* BYE {}\r\n", message),
            Response::Untagged { message } => write!(f, "* {}\r\n", message),
            Response::Continuation { message } => write!(f, "+ {}\r\n", message),
        }
    }
}

/// Parse an IMAP command from a line of input
pub fn parse_command(tag: &str, line: &str) -> Result<Command> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.is_empty() {
        return Err(Error::ProtocolError("Empty command".to_string()));
    }

    let cmd = parts[0].to_uppercase();
    match cmd.as_str() {
        "CAPABILITY" => Ok(Command::Capability),
        "NOOP" => Ok(Command::Noop),
        "LOGOUT" => Ok(Command::Logout),
        "LOGIN" => {
            if parts.len() < 3 {
                return Err(Error::ProtocolError("LOGIN requires username and password".to_string()));
            }
            Ok(Command::Login {
                username: parts[1].trim_matches('"').to_string(),
                password: parts[2].trim_matches('"').to_string(),
            })
        }
        "SELECT" => {
            if parts.len() < 2 {
                return Err(Error::ProtocolError("SELECT requires mailbox name".to_string()));
            }
            Ok(Command::Select {
                mailbox: parts[1].trim_matches('"').to_string(),
            })
        }
        "EXAMINE" => {
            if parts.len() < 2 {
                return Err(Error::ProtocolError("EXAMINE requires mailbox name".to_string()));
            }
            Ok(Command::Examine {
                mailbox: parts[1].trim_matches('"').to_string(),
            })
        }
        "CREATE" => {
            if parts.len() < 2 {
                return Err(Error::ProtocolError("CREATE requires mailbox name".to_string()));
            }
            Ok(Command::Create {
                mailbox: parts[1].trim_matches('"').to_string(),
            })
        }
        "DELETE" => {
            if parts.len() < 2 {
                return Err(Error::ProtocolError("DELETE requires mailbox name".to_string()));
            }
            Ok(Command::Delete {
                mailbox: parts[1].trim_matches('"').to_string(),
            })
        }
        "LIST" => {
            if parts.len() < 3 {
                return Err(Error::ProtocolError("LIST requires reference and pattern".to_string()));
            }
            Ok(Command::List {
                reference: parts[1].trim_matches('"').to_string(),
                pattern: parts[2].trim_matches('"').to_string(),
            })
        }
        "CLOSE" => Ok(Command::Close),
        "EXPUNGE" => Ok(Command::Expunge),
        _ => Err(Error::ProtocolError(format!("Unknown command: {}", cmd))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_capability() {
        let cmd = parse_command("A001", "CAPABILITY").unwrap();
        assert_eq!(cmd, Command::Capability);
    }

    #[test]
    fn test_parse_login() {
        let cmd = parse_command("A002", "LOGIN username password").unwrap();
        assert_eq!(cmd, Command::Login {
            username: "username".to_string(),
            password: "password".to_string(),
        });
    }

    #[test]
    fn test_parse_select() {
        let cmd = parse_command("A003", "SELECT INBOX").unwrap();
        assert_eq!(cmd, Command::Select {
            mailbox: "INBOX".to_string(),
        });
    }

    #[test]
    fn test_response_format() {
        let resp = Response::Ok {
            tag: Some("A001".to_string()),
            message: "CAPABILITY completed".to_string(),
        };
        assert_eq!(resp.to_string(), "A001 OK CAPABILITY completed\r\n");
    }
}
