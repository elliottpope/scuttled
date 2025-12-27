//! Core types used throughout the IMAP server

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a message
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(pub Uuid);

impl MessageId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for MessageId {
    fn default() -> Self {
        Self::new()
    }
}

/// Unique identifier for a mailbox
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MailboxId {
    pub username: Username,
    pub name: MailboxName,
}

/// Message UID (unique within a mailbox)
pub type Uid = u32;

/// Message sequence number (changes as messages are deleted)
pub type SequenceNumber = u32;

/// Mailbox name
pub type MailboxName = String;

/// Username
pub type Username = String;

/// Represents an email message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub uid: Uid,
    pub mailbox: MailboxName,
    pub flags: Vec<MessageFlag>,
    pub internal_date: DateTime<Utc>,
    pub size: usize,
    pub raw_content: Vec<u8>,
}

/// Message flags as defined by IMAP
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageFlag {
    Seen,
    Answered,
    Flagged,
    Deleted,
    Draft,
    Recent,
    Custom(String),
}

impl MessageFlag {
    pub fn to_imap_string(&self) -> String {
        match self {
            MessageFlag::Seen => "\\Seen".to_string(),
            MessageFlag::Answered => "\\Answered".to_string(),
            MessageFlag::Flagged => "\\Flagged".to_string(),
            MessageFlag::Deleted => "\\Deleted".to_string(),
            MessageFlag::Draft => "\\Draft".to_string(),
            MessageFlag::Recent => "\\Recent".to_string(),
            MessageFlag::Custom(s) => s.clone(),
        }
    }

    pub fn from_imap_string(s: &str) -> Self {
        match s {
            "\\Seen" => MessageFlag::Seen,
            "\\Answered" => MessageFlag::Answered,
            "\\Flagged" => MessageFlag::Flagged,
            "\\Deleted" => MessageFlag::Deleted,
            "\\Draft" => MessageFlag::Draft,
            "\\Recent" => MessageFlag::Recent,
            _ => MessageFlag::Custom(s.to_string()),
        }
    }
}

/// Mailbox information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mailbox {
    pub name: MailboxName,
    pub uid_validity: u32,
    pub uid_next: Uid,
    pub flags: Vec<MessageFlag>,
    pub permanent_flags: Vec<MessageFlag>,
    pub message_count: usize,
    pub recent_count: usize,
    pub unseen_count: usize,
}

/// User credentials
#[derive(Debug, Clone)]
pub struct Credentials {
    pub username: Username,
    pub password: String,
}

/// User information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub username: Username,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
}

/// Search query for finding messages
#[derive(Debug, Clone)]
pub enum SearchQuery {
    All,
    Text(String),
    From(String),
    To(String),
    Subject(String),
    Body(String),
    Uid(Vec<Uid>),
    Sequence(Vec<SequenceNumber>),
    Seen,
    Unseen,
    Flagged,
    Unflagged,
    Deleted,
    Undeleted,
    And(Box<SearchQuery>, Box<SearchQuery>),
    Or(Box<SearchQuery>, Box<SearchQuery>),
    Not(Box<SearchQuery>),
}

/// Task for the queue system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueTask {
    pub id: Uuid,
    pub task_type: TaskType,
    pub created_at: DateTime<Utc>,
    pub payload: serde_json::Value,
}

/// Types of tasks that can be queued
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskType {
    UidCompaction,
    IndexUpdate,
    Expunge,
    Custom(String),
}

impl QueueTask {
    pub fn new(task_type: TaskType, payload: serde_json::Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            task_type,
            created_at: Utc::now(),
            payload,
        }
    }
}
